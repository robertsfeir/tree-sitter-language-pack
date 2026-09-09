//! SQL DDL declaration extraction.
//!
//! The SQL grammar parses each `CREATE` statement into a `create_*` node, so a
//! declaration is classified by node kind and named from its `object_reference`.
//! Two things make this more than a kind table.
//!
//! PostgreSQL files carry statements the grammar does not know (`GRANT`, `DO`
//! blocks, psql meta-commands, `SET search_path` in a function header), and
//! tree-sitter's error recovery then folds the next statement, sometimes the
//! rest of the file, into an `ERROR` node whose children are that statement's
//! own tokens. The declaration is still there, keyword by keyword, so
//! [`recovered`] reads it back from the token run. Measured on 124 PostgreSQL
//! files, 150 of roughly 550 `CREATE` statements sat inside `ERROR` nodes.
//!
//! The generic walk must not run on SQL at all: the grammar names a plpgsql
//! `DECLARE` variable `function_declaration`, which the language-neutral
//! matcher reads as a function. One proof file produced 240 such items.
//!
//! Entry point: [`structure`]. Nothing here walks below a matched declaration,
//! so a `CREATE` inside a function body is part of the function, not an item.

use tree_sitter::Node;

use super::intelligence::node_text;
use super::types::*;
use super::walk::{Descend, walk_bounded, warn_if_truncated};

/// Every SQL declaration in `root`, parsed or recovered, in source order.
pub(super) fn structure(root: &Node<'_>, source: &str) -> Vec<StructureItem> {
    let mut items = Vec::new();
    let truncated = walk_bounded(root, |node, _depth| {
        if let Some(item) = declaration(node, source) {
            items.push(item);
            return Descend::Skip;
        }
        if node.is_error() {
            items.extend(recovered(node, source));
        }
        Descend::Children
    });
    warn_if_truncated(truncated, "intel::sql", "sql");
    items
}

/// The declaration a parsed `create_*` node makes, spanning the statement and
/// its terminator.
fn declaration(node: &Node<'_>, source: &str) -> Option<StructureItem> {
    if !matches!(
        node.kind(),
        "create_table"
            | "create_view"
            | "create_materialized_view"
            | "create_function"
            | "create_procedure"
            | "create_type"
            | "create_index"
            | "create_trigger"
            | "create_policy"
            | "create_schema"
            | "create_database"
            | "create_role"
    ) {
        return None;
    }
    let mut cursor = node.walk();
    let tokens: Vec<Node<'_>> = node.children(&mut cursor).collect();
    let head = read_head(&tokens, source)?;
    // ~keep The terminator is the statement's sibling, not the create node's child.
    let end = node
        .parent()
        .filter(|parent| parent.kind() == "statement")
        .and_then(|parent| parent.next_sibling())
        .filter(|sibling| sibling.kind() == ";")
        .map_or(node.end_byte(), |terminator| terminator.end_byte());
    Some(item(head, node.start_byte(), end, source))
}

/// Declarations whose tokens error recovery left as direct children of an
/// `ERROR` node. A run starts at a `keyword_create` child and ends at the
/// statement's terminator, found by [`recovered_end`].
fn recovered(error: &Node<'_>, source: &str) -> Vec<StructureItem> {
    let mut cursor = error.walk();
    let tokens: Vec<Node<'_>> = error.children(&mut cursor).collect();
    let mut items = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind() != "keyword_create" {
            continue;
        }
        let Some(head) = read_head(&tokens[index..], source) else {
            continue;
        };
        let after_name = index + head.consumed;
        let Some(end) = recovered_end(&tokens, after_name, head.routine) else {
            continue;
        };
        items.push(item(head, token.start_byte(), end, source));
    }
    items
}

/// The classified head of a `CREATE` statement: its kind and qualified name,
/// plus how many tokens the head consumed.
struct Head {
    kind: StructureKind,
    name: String,
    consumed: usize,
    /// A function or procedure, whose body is dollar-quoted.
    routine: bool,
}

/// Read `CREATE [OR REPLACE] [modifiers] <object> [IF NOT EXISTS] <name>` from
/// `tokens`, which start at the `keyword_create` token. Triggers and policies
/// are qualified by the table they are declared `ON`, because PostgreSQL scopes
/// their names to that table.
fn read_head(tokens: &[Node<'_>], source: &str) -> Option<Head> {
    let mut index = 1;
    let mut object = None;
    while index < tokens.len() {
        let kind = tokens[index].kind();
        index += 1;
        match kind {
            "keyword_or"
            | "keyword_replace"
            | "keyword_unique"
            | "keyword_temp"
            | "keyword_temporary"
            | "keyword_materialized"
            | "keyword_unlogged" => {}
            keyword if keyword.starts_with("keyword_") => {
                object = Some(keyword);
                break;
            }
            _ => return None,
        }
    }
    let object = object?;
    let kind = match object {
        "keyword_table" | "keyword_view" => StructureKind::Struct,
        "keyword_function" | "keyword_procedure" => StructureKind::Function,
        "keyword_type" => StructureKind::Other("Type".to_string()),
        "keyword_index" => StructureKind::Other("Index".to_string()),
        "keyword_trigger" => StructureKind::Other("Trigger".to_string()),
        "keyword_policy" => StructureKind::Other("Policy".to_string()),
        "keyword_schema" | "keyword_database" => StructureKind::Module,
        "keyword_role" => StructureKind::Other("Role".to_string()),
        _ => return None,
    };
    while index < tokens.len()
        && matches!(
            tokens[index].kind(),
            "keyword_if" | "keyword_not" | "keyword_exists" | "keyword_concurrently"
        )
    {
        index += 1;
    }
    let mut name = object_name(tokens.get(index)?, source)?;
    index += 1;
    if matches!(object, "keyword_trigger" | "keyword_policy") {
        let on = tokens[index..].iter().position(|token| token.kind() == "keyword_on")? + index;
        let table = object_name(tokens.get(on + 1)?, source)?;
        name = format!("{table}.{name}");
        index = on + 2;
    }
    Some(Head {
        kind,
        name,
        consumed: index,
        routine: matches!(object, "keyword_function" | "keyword_procedure"),
    })
}

/// The name an object token carries, schema-qualified when the reference is,
/// with the double quotes of a quoted identifier removed. A psql variable
/// (`:db_name`, `:"role"`) parses as an `ERROR` colon and is not a name, and
/// a keyword where the name should be means the name was lost to recovery.
fn object_name(token: &Node<'_>, source: &str) -> Option<String> {
    match token.kind() {
        "object_reference" => {
            let name = plain(node_text(&token.child_by_field_name("name")?, source));
            Some(match token.child_by_field_name("schema") {
                Some(schema) => format!("{}.{name}", plain(node_text(&schema, source))),
                None => name.to_string(),
            })
        }
        "identifier" => Some(plain(node_text(token, source)).to_string()),
        "field" => object_name(&token.child_by_field_name("name")?, source),
        "invocation" => {
            let mut cursor = token.walk();
            let reference = token
                .named_children(&mut cursor)
                .find(|child| child.kind() == "object_reference")?;
            object_name(&reference, source)
        }
        _ => None,
    }
}

fn plain(identifier: &str) -> &str {
    identifier
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(identifier)
}

/// Where a recovered statement ends: at the dollar quote closing a routine's
/// body, else at the first `;` token, stopping short of the next parsed
/// statement or `CREATE` keyword so a lost terminator cannot pull the span
/// over its neighbour.
fn recovered_end(tokens: &[Node<'_>], from: usize, routine: bool) -> Option<usize> {
    if routine && let Some(open) = leaves(&tokens[from..]).find(|leaf| leaf.kind() == "dollar_quote") {
        let tag = open.byte_range();
        let mut after_open = leaves(&tokens[from..]).skip_while(|leaf| leaf.id() != open.id());
        after_open.next();
        if let Some(close) =
            after_open.find(|leaf| leaf.kind() == "dollar_quote" && leaf.byte_range().len() == tag.len())
        {
            return Some(close.end_byte());
        }
    }
    let mut end = None;
    for token in &tokens[from..] {
        if matches!(token.kind(), "statement" | "keyword_create") {
            break;
        }
        if let Some(terminator) = leaves(std::slice::from_ref(token)).find(|leaf| leaf.kind() == ";") {
            return Some(terminator.end_byte());
        }
        end = Some(token.end_byte());
    }
    end
}

/// The leaf tokens under `tokens`, in source order.
fn leaves<'tree>(tokens: &[Node<'tree>]) -> impl Iterator<Item = Node<'tree>> {
    let mut pending: Vec<Node<'tree>> = tokens.iter().rev().copied().collect();
    std::iter::from_fn(move || {
        while let Some(node) = pending.pop() {
            if node.child_count() == 0 {
                return Some(node);
            }
            let mut cursor = node.walk();
            let children: Vec<Node<'tree>> = node.children(&mut cursor).collect();
            pending.extend(children.into_iter().rev());
        }
        None
    })
}

fn item(head: Head, start: usize, end: usize, source: &str) -> StructureItem {
    StructureItem {
        kind: head.kind,
        name: Some(head.name),
        span: super::intelligence::span_between(source, start, end),
        ..StructureItem::default()
    }
}
