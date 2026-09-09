//! Declaration coverage for the code-shaped repository formats. Their grammars
//! name nodes the generic matcher misreads, so each has its own adapter,
//! exercised here on the shapes real repository files take.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use tree_sitter_language_pack::{ProcessConfig, ProcessResult, StructureItem, StructureKind, process};

fn extract(source: &str, language: &str) -> ProcessResult {
    process(source, &ProcessConfig::new(language).all()).expect("real grammar required; build with TSLP_LANGUAGES=sql")
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
