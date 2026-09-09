//! Declaration coverage for the code-shaped repository formats: SQL DDL,
//! Justfiles and Dockerfiles. Their grammars name nodes the generic matcher
//! misreads, so each has its own adapter, exercised here on the shapes real
//! PostgreSQL bootstrap scripts, Justfiles and multi-stage Dockerfiles take.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use tree_sitter_language_pack::{ProcessConfig, ProcessResult, StructureItem, StructureKind, process};

fn extract(source: &str, language: &str) -> ProcessResult {
    process(source, &ProcessConfig::new(language).all())
        .expect("real grammar required; build with TSLP_LANGUAGES=sql,just,dockerfile")
}

/// (name, kind, start_line, end_line) for each item, top level only.
fn summary(items: &[StructureItem]) -> Vec<(Option<&str>, &StructureKind, usize, usize)> {
    items
        .iter()
        .map(|item| {
            (
                item.name.as_deref(),
                &item.kind,
                item.span.start_line,
                item.span.end_line,
            )
        })
        .collect()
}

fn other(label: &str) -> StructureKind {
    StructureKind::Other(label.to_string())
}

const SQL_DDL: &str = "\
-- comment
CREATE SCHEMA IF NOT EXISTS engram;
CREATE EXTENSION IF NOT EXISTS vector;
CREATE ROLE engram_app LOGIN;
CREATE TABLE engram.thoughts (
  id uuid PRIMARY KEY,
  tenant_id uuid NOT NULL
);
CREATE TABLE IF NOT EXISTS \"Quoted\".\"My Table\" (id int);
CREATE UNIQUE INDEX thoughts_tenant_idx ON engram.thoughts (tenant_id, id);
CREATE INDEX CONCURRENTLY IF NOT EXISTS other_idx ON thoughts USING btree (id);
CREATE OR REPLACE FUNCTION engram_current_tenant()
RETURNS uuid
LANGUAGE sql STABLE AS $$
  SELECT current_setting('engram.tenant_id', true)::uuid
$$;
CREATE FUNCTION public.sync_lexemes() RETURNS trigger LANGUAGE plpgsql AS $body$
BEGIN
  INSERT INTO x VALUES (1);
  RETURN NEW;
END;
$body$;
CREATE TRIGGER engram_sync_symbol_lexemes AFTER INSERT OR UPDATE ON symbol_cards
  FOR EACH ROW EXECUTE FUNCTION sync_lexemes();
CREATE POLICY tenant_isolation ON engram.thoughts
  USING (tenant_id = engram_current_tenant());
CREATE OR REPLACE VIEW engram.recent AS SELECT * FROM engram.thoughts;
CREATE MATERIALIZED VIEW engram.stats AS SELECT count(*) FROM engram.thoughts;
CREATE TYPE mood AS ENUM ('sad', 'ok');
CREATE TEMP TABLE scratch (id int);
CREATE SEQUENCE seq1;
ALTER TABLE engram.thoughts ENABLE ROW LEVEL SECURITY;
SELECT 1;
";

#[test]
fn sql_ddl_objects_have_kinds_qualified_names_and_statement_spans() {
    let result = extract(SQL_DDL, "sql");
    assert_eq!(
        summary(&result.structure),
        vec![
            (Some("engram"), &StructureKind::Module, 1, 1),
            (Some("engram_app"), &other("Role"), 3, 3),
            (Some("engram.thoughts"), &StructureKind::Struct, 4, 7),
            (Some("Quoted.My Table"), &StructureKind::Struct, 8, 8),
            (Some("thoughts_tenant_idx"), &other("Index"), 9, 9),
            (Some("other_idx"), &other("Index"), 10, 10),
            (Some("engram_current_tenant"), &StructureKind::Function, 11, 15),
            (Some("public.sync_lexemes"), &StructureKind::Function, 16, 21),
            (
                Some("symbol_cards.engram_sync_symbol_lexemes"),
                &other("Trigger"),
                22,
                23
            ),
            (Some("engram.thoughts.tenant_isolation"), &other("Policy"), 24, 25),
            (Some("engram.recent"), &StructureKind::Struct, 26, 26),
            (Some("engram.stats"), &StructureKind::Struct, 27, 27),
            (Some("mood"), &other("Type"), 28, 28),
            (Some("scratch"), &StructureKind::Struct, 29, 29),
        ]
    );
    // The statement span includes its terminator and nothing after it.
    let function = &result.structure[6].span;
    assert!(SQL_DDL[function.start_byte..function.end_byte].ends_with("$$;"));
    assert!(result.structure.iter().all(|item| item.children.is_empty()));
    assert!(result.symbols.is_empty(), "SQL emits no flat symbols");
}

#[test]
fn sql_declarations_folded_into_error_recovery_keep_exact_spans() {
    // `SET search_path` in a function header and the GRANT are not in the
    // grammar; recovery folds everything after each into an ERROR node.
    let source = "\
CREATE OR REPLACE FUNCTION engram_purge(p_tenant text)
RETURNS jsonb
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
  n_cards integer;
BEGIN
  DELETE FROM symbol_cards WHERE tenant_id = p_tenant;
  RETURN jsonb_build_object('cards', n_cards);
END;
$fn$;
GRANT SELECT ON symbol_cards TO engram_app;
CREATE UNIQUE INDEX symbol_cards_id_key ON symbol_cards (id);
CREATE POLICY symbol_cards_rls ON symbol_cards
  USING (tenant_id = engram_current_tenant());
CREATE OR REPLACE FUNCTION engram_sync() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RETURN NEW;
END;
$$;
CREATE TABLE code_repos (id uuid PRIMARY KEY);
";
    let result = extract(source, "sql");
    assert!(result.metrics.error_count > 0, "the fixture must exercise recovery");
    assert_eq!(
        summary(&result.structure),
        vec![
            (Some("engram_purge"), &StructureKind::Function, 0, 12),
            (Some("symbol_cards_id_key"), &other("Index"), 14, 14),
            (Some("symbol_cards.symbol_cards_rls"), &other("Policy"), 15, 16),
            (Some("engram_sync"), &StructureKind::Function, 17, 21),
            (Some("code_repos"), &StructureKind::Struct, 22, 22),
        ]
    );
    for item in &result.structure {
        let text = &source[item.span.start_byte..item.span.end_byte];
        assert!(text.starts_with("CREATE"), "{text}");
        assert!(text.ends_with(';'), "{text}");
    }
}

#[test]
fn sql_lost_names_variables_and_bodies_are_not_declarations() {
    // A CREATE inside a recognised body belongs to the routine. A psql
    // variable, a keyword where the name should be (GRANT CREATE ON), and a
    // format placeholder are not names, so those statements yield nothing.
    let source = "\
CREATE OR REPLACE FUNCTION counted() RETURNS integer LANGUAGE plpgsql AS $$
DECLARE
  v_total integer;
BEGIN
  CREATE TEMP TABLE inner_scratch (id int);
  SELECT count(*) INTO v_total FROM symbol_cards;
  RETURN v_total;
END;
$$;
GRANT CREATE ON SCHEMA public TO engram;
CREATE DATABASE :db_name OWNER engram;
CREATE ROLE :\"role_name\" LOGIN;
CREATE FUNCTION %s(%s) RETURNS %s;
";
    let result = extract(source, "sql");
    assert!(result.metrics.error_count > 0, "the fixture must exercise recovery");
    assert_eq!(
        summary(&result.structure),
        vec![(Some("counted"), &StructureKind::Function, 0, 8)]
    );
}

#[test]
fn sql_recovery_reads_a_schema_the_lexer_handed_over_as_a_keyword() {
    // Shrunk from a real proof file: the first body derails the parser, which
    // runs the function past its own closing quote and hands the next header
    // over as `keyword_public`, `.`, then the bare name.
    let source = "\
    CREATE OR REPLACE FUNCTION public.engram_floor_complete(p_tenant text, p_set_id uuid)
     RETURNS boolean
    AS $function$
    DECLARE
      -- SQLSTATE 55P03 here instead of parking this tenant connection.
                 AND  sv.card_id   = sc.id
                 AND  sv.set_id    = sc.set_id
             );

      RETURN v_floorless = 0;
    END;
    $function$
;
    CREATE OR REPLACE FUNCTION public.engram_complete(p_tenant text, p_floor_complete boolean)
     RETURNS text
     LANGUAGE plpgsql
     SECURITY DEFINER
     SET search_path TO 'public', 'pg_catalog'
    AS $function$
    DECLARE
      v_repo             text;
    BEGIN
      RETURN 1;
    END;
    $function$
;
";
    let result = extract(source, "sql");
    assert_eq!(
        summary(&result.structure),
        vec![
            (Some("public.engram_floor_complete"), &StructureKind::Function, 0, 12),
            (Some("public.engram_complete"), &StructureKind::Function, 13, 25),
        ]
    );
    for item in &result.structure {
        let text = &source[item.span.start_byte..item.span.end_byte];
        assert!(text.starts_with("CREATE") && text.ends_with("$function$\n;"), "{text}");
    }
}

#[test]
fn sql_table_the_parser_ran_past_its_terminator_ends_there() {
    // Shrunk from a real DDL proof: the REFERENCES clause and the GRANTs after
    // it fold the next CREATE TABLE into the first table's node, and the lexer
    // drops that second table's name token, so it yields nothing rather than
    // a name it never had.
    let source = "\
CREATE TABLE tinker_platform_release_files (
  platform_file_id     uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
  platform_release_id  uuid        NOT NULL
    REFERENCES tinker_platform_releases (platform_release_id) ON DELETE CASCADE,
  CONSTRAINT tinker_platform_release_files_path_key UNIQUE (platform_release_id, rel_path)
);
GRANT SELECT, INSERT, UPDATE, DELETE ON tinker_platform_release_files TO kyroco_publisher;
REVOKE INSERT, UPDATE, DELETE ON tinker_platform_release_files FROM engram_app;
CREATE TABLE tinker_promotion_audit (
  audit_id        uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
  CONSTRAINT tinker_promotion_audit_direction_chk
    CHECK (direction IN ('promote','demote'))
);
";
    let result = extract(source, "sql");
    assert_eq!(
        summary(&result.structure),
        vec![(Some("tinker_platform_release_files"), &StructureKind::Struct, 0, 5)]
    );
    let table = &result.structure[0].span;
    assert!(source[table.start_byte..table.end_byte].ends_with(");"));
}

const JUSTFILE: &str = "\
set shell := [\"bash\", \"-eu\", \"-c\"]

mod kyroco 'justfiles/kyroco.just'
import 'other.just'

MIX := \"/opt/homebrew/bin/mix\"

# Run the proof.
[private]
default:
    @just --list

pre-pr *args:
    cd {{DIR}} && ./scripts/pre-pr.sh {{args}}

alias pp := pre-pr

[group(\"ci\")]
@build target=\"debug\" flag: deps compile
    echo {{target}}
    echo done

_hidden:
    echo hidden

deps:
    mix deps.get
compile: deps
    mix compile
";

#[test]
fn justfile_recipes_aliases_and_modules_are_declarations_with_trimmed_spans() {
    let result = extract(JUSTFILE, "just");
    assert_eq!(result.metrics.error_count, 0);
    assert_eq!(
        summary(&result.structure),
        vec![
            (Some("kyroco"), &StructureKind::Module, 2, 2),
            (Some("default"), &StructureKind::Function, 8, 10),
            (Some("pre-pr"), &StructureKind::Function, 12, 13),
            (Some("pp"), &other("Alias"), 15, 15),
            (Some("build"), &StructureKind::Function, 17, 20),
            (Some("_hidden"), &StructureKind::Function, 22, 23),
            (Some("deps"), &StructureKind::Function, 25, 26),
            (Some("compile"), &StructureKind::Function, 27, 28),
        ]
    );
    let build = &result.structure[4].span;
    assert!(JUSTFILE[build.start_byte..build.end_byte].starts_with("[group("));
    assert!(JUSTFILE[build.start_byte..build.end_byte].ends_with("echo done"));
    assert!(result.symbols.is_empty(), "Just emits no flat symbols");
}

const DOCKERFILE: &str = "\
# syntax=docker/dockerfile:1
ARG ERLANG_VERSION=29.0.3
ARG DEBIAN_VERSION=bookworm
FROM hexpm/erlang:${ERLANG_VERSION}-debian-${DEBIAN_VERSION} AS toolchain_base
ARG ELIXIR_VERSION
ENV MIX_ENV=${MIX_ENV} \\
    LANG=C.UTF-8
RUN apt-get update \\
  && apt-get install -y git
COPY . /app

# The next stage.
FROM toolchain_base AS deps_base
WORKDIR /app
ENV RUSTUP_HOME=/opt/rustup
FROM debian:${DEBIAN_VERSION}-slim as runtime_api
ARG MIX_ENV
ENV LANG C.UTF-8
FROM nginx:1.27-alpine
ARG MIX_ENV
CMD [\"nginx\"]
";

#[test]
fn dockerfile_named_stages_nest_their_args_and_env_pairs() {
    let result = extract(DOCKERFILE, "dockerfile");
    assert_eq!(result.metrics.error_count, 0);
    assert_eq!(
        summary(&result.structure),
        vec![
            (Some("ERLANG_VERSION"), &other("Constant"), 1, 1),
            (Some("DEBIAN_VERSION"), &other("Constant"), 2, 2),
            (Some("toolchain_base"), &StructureKind::Module, 3, 9),
            (Some("deps_base"), &StructureKind::Module, 12, 14),
            (Some("runtime_api"), &StructureKind::Module, 15, 17),
            (Some("MIX_ENV"), &other("Constant"), 19, 19),
        ]
    );
    assert_eq!(
        summary(&result.structure[2].children),
        vec![
            (Some("ELIXIR_VERSION"), &other("Constant"), 4, 4),
            (Some("MIX_ENV"), &other("Constant"), 5, 6),
            (Some("LANG"), &other("Constant"), 5, 6),
        ]
    );
    assert_eq!(
        summary(&result.structure[3].children),
        vec![(Some("RUSTUP_HOME"), &other("Constant"), 14, 14)]
    );
    assert_eq!(
        summary(&result.structure[4].children),
        vec![
            (Some("MIX_ENV"), &other("Constant"), 16, 16),
            (Some("LANG"), &other("Constant"), 17, 17),
        ]
    );
    let stage = &result.structure[2].span;
    assert!(DOCKERFILE[stage.start_byte..stage.end_byte].ends_with("COPY . /app"));
    assert!(result.symbols.is_empty(), "Dockerfile emits no flat symbols");
}
