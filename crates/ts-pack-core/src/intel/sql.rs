//! SQL DDL declaration extraction.
//!
//! The SQL grammar parses each `CREATE` statement into a `create_*` node, so a
//! declaration is classified by node kind and named from its tokens. Two things
//! make this more than a kind table.
//!
//! PostgreSQL files carry statements the grammar does not know (`GRANT`, `DO`
//! blocks, psql meta-commands, `SET search_path` in a function header), and
//! tree-sitter's error recovery then folds the next statement, sometimes the
//! rest of the file, into `ERROR` nodes that keep the statement's tokens but
//! not its node. The declaration is still there, keyword by keyword, so the
//! second pass reads every `CREATE` outside a parsed declaration back from the
//! leaf token stream. Measured on 124 PostgreSQL files, 150 of roughly 550
//! `CREATE` statements sat inside `ERROR` nodes.
//!
//! The generic walk must not run on SQL at all: the grammar names a plpgsql
//! `DECLARE` variable `function_declaration`, which the language-neutral
//! matcher reads as a function. One proof file produced 240 such items.
//!
//! Entry point: [`structure`]. Nothing here looks below a matched declaration,
//! so a `CREATE` inside a function body is part of the function, not an item.

use tree_sitter::Node;

use super::intelligence::{node_text, span_between};
use super::types::*;
use super::walk::{Descend, walk_bounded, warn_if_truncated};

/// Every SQL declaration in `root`, parsed or recovered, in source order.
pub(super) fn structure(root: &Node<'_>, source: &str) -> Vec<StructureItem> {
    let mut items = Vec::new();
    let mut parsed: Vec<(usize, usize)> = Vec::new();
    let truncated = walk_bounded(root, |node, _depth| {
        if !node.kind().starts_with("create_") {
            return Descend::Children;
        }
        if let Some(item) = declaration(node, source) {
            parsed.push((item.span.start_byte, item.span.end_byte));
            items.push(item);
        }
        Descend::Skip
    });
    warn_if_truncated(truncated, "intel::sql", "sql");

    let leaves: Vec<Node<'_>> = leaves(root).collect();
    for (index, leaf) in leaves.iter().enumerate() {
        if leaf.kind() != "keyword_create"
            || parsed
                .iter()
                .any(|(start, end)| (start..end).contains(&&leaf.start_byte()))
        {
            continue;
        }
        let Some(head) = read_head(&leaves, index, source) else {
            continue;
        };
        let Some(end) = recovered_end(&leaves, head.consumed, head.routine) else {
            continue;
        };
        items.push(item(head, leaf.start_byte(), end, source));
    }
    items.sort_by_key(|item| item.span.start_byte);
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
    let leaves: Vec<Node<'_>> = leaves(node).collect();
    let head = read_head(&leaves, 0, source)?;
    // ~keep The terminator is the statement's sibling, not the create node's child.
    let after_node = node
        .parent()
        .filter(|parent| parent.kind() == "statement")
        .and_then(|parent| parent.next_sibling())
        .filter(|sibling| sibling.kind() == ";")
        .map_or(node.end_byte(), |terminator| terminator.end_byte());
    // ~keep Error recovery can run a routine's body past its own closing quote and
    // ~keep swallow the next statement into it. The body ends at that quote, and
    // ~keep the swallowed statement is read back by the recovery pass.
    let end = match head.routine.then(|| routine_close(&leaves, head.consumed)).flatten() {
        Some(close) if close + 1 < leaves.len() => terminated(&leaves, close),
        _ => after_node,
    };
    Some(item(head, node.start_byte(), end, source))
}

/// The classified head of a `CREATE` statement: its kind and qualified name,
/// plus the index of the first leaf after the head.
struct Head {
    kind: StructureKind,
    name: String,
    consumed: usize,
    /// A function or procedure, whose body is dollar-quoted.
    routine: bool,
}

/// Read `CREATE [OR REPLACE] [modifiers] <object> [IF NOT EXISTS] <name>` from
/// the leaves starting at `start`, a `keyword_create`. Triggers and policies
/// are qualified by the table they are declared `ON`, because PostgreSQL
/// scopes their names to that table.
fn read_head(leaves: &[Node<'_>], start: usize, source: &str) -> Option<Head> {
    let mut index = start + 1;
    let mut object = None;
    while index < leaves.len() {
        let kind = leaves[index].kind();
        index += 1;
        match kind {
            "comment"
            | "keyword_or"
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
    while index < leaves.len()
        && matches!(
            leaves[index].kind(),
            "comment" | "keyword_if" | "keyword_not" | "keyword_exists" | "keyword_concurrently"
        )
    {
        index += 1;
    }
    let mut name = qualified_name(leaves, &mut index, source)?;
    if matches!(object, "keyword_trigger" | "keyword_policy") {
        index = leaves[index..].iter().position(|leaf| leaf.kind() == "keyword_on")? + index + 1;
        let table = qualified_name(leaves, &mut index, source)?;
        name = format!("{table}.{name}");
    }
    Some(Head {
        kind,
        name,
        consumed: index,
        routine: matches!(object, "keyword_function" | "keyword_procedure"),
    })
}

/// The dotted name at `leaves[*index]`, advancing past it: identifiers joined
/// by dots, with the double quotes of a quoted identifier removed. A schema
/// that is also a keyword (`public.name`) arrives as `keyword_public` before
/// the dot, so a keyword followed by a dot is a name part. A psql variable
/// (`:db_name`, `:"role"`) is an error colon before the identifier and is not
/// a name; a keyword where the name should be means the name was lost.
fn qualified_name(leaves: &[Node<'_>], index: &mut usize, source: &str) -> Option<String> {
    if leaves
        .get(*index - 1)
        .is_some_and(|before| node_text(before, source) == ":")
    {
        return None;
    }
    let mut parts = Vec::new();
    while let Some(leaf) = leaves.get(*index) {
        let dotted = leaves.get(*index + 1).is_some_and(|next| next.kind() == ".");
        if leaf.kind() != "identifier" && !(leaf.kind().starts_with("keyword_") && dotted) {
            break;
        }
        parts.push(plain(node_text(leaf, source)));
        *index += 1;
        if !dotted {
            break;
        }
        *index += 1;
    }
    if parts.is_empty() { None } else { Some(parts.join(".")) }
}

fn plain(identifier: &str) -> &str {
    identifier
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(identifier)
}

/// Where a recovered statement ends: at the dollar quote closing a routine's
/// body, else at the first `;`, stopping short of the next `CREATE` so a lost
/// terminator cannot pull the span over its neighbour.
fn recovered_end(leaves: &[Node<'_>], from: usize, routine: bool) -> Option<usize> {
    if routine && let Some(close) = routine_close(leaves, from) {
        return Some(terminated(leaves, close));
    }
    let mut end = None;
    for leaf in &leaves[from..] {
        match leaf.kind() {
            ";" => return Some(leaf.end_byte()),
            "keyword_create" => break,
            _ => end = Some(leaf.end_byte()),
        }
    }
    end
}

/// The index of the dollar quote closing the body that opens after `from`:
/// the next quote of the same length, since PostgreSQL does not nest a tag
/// inside itself.
fn routine_close(leaves: &[Node<'_>], from: usize) -> Option<usize> {
    let quote = |leaf: &Node<'_>| leaf.kind() == "dollar_quote" && !leaf.byte_range().is_empty();
    let open = from + leaves[from..].iter().position(quote)?;
    let tag = leaves[open].byte_range().len();
    let close = leaves[open + 1..]
        .iter()
        .position(|leaf| quote(leaf) && leaf.byte_range().len() == tag)?;
    Some(open + 1 + close)
}

/// The end of the statement whose last token is `leaves[index]`, taking in the
/// terminator when it is the next leaf.
fn terminated(leaves: &[Node<'_>], index: usize) -> usize {
    match leaves.get(index + 1) {
        Some(next) if next.kind() == ";" => next.end_byte(),
        _ => leaves[index].end_byte(),
    }
}

/// The leaf tokens under `root`, in source order, without native recursion.
fn leaves<'tree>(root: &Node<'tree>) -> impl Iterator<Item = Node<'tree>> {
    let mut pending: Vec<Node<'tree>> = vec![*root];
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
        span: span_between(source, start, end),
        ..StructureItem::default()
    }
}
