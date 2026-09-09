//! C declaration extraction, for sources and headers.
//!
//! The C grammar names a definition and a prototype differently: a
//! `function_definition` has a body, while a prototype is a `declaration`
//! whose declarator is a `function_declarator` applied directly to an
//! identifier. Both are functions here, because a header's prototypes are the
//! declarations a reader of that header is looking for. A declaration whose
//! function declarator wraps a parenthesised pointer (`int (*f)(int);`) is a
//! function pointer variable, and variables declare nothing here.
//!
//! A `struct`, `union` or `enum` is an item only where it is defined, which
//! the grammar marks with a `body`, and only where it has a name: a nameless
//! `typedef struct { ... } name_t;` is the typedef alone. A definition that
//! sits inside a `typedef` or a variable declaration is read out of the
//! `type` field, so `typedef struct node { ... } node_t;` is two items with
//! two names. Typedefs are `Other("Type")`. An object-like macro is
//! `Other("Constant")` and a function-like one `Other("Macro")`; macro bodies
//! never reach an item. Fields, enumerators, includes and anything inside a
//! function body produce nothing. Preprocessor conditionals and `extern "C"`
//! blocks are transparent, so a header guarded by `#ifndef` reads as its
//! contents.
//!
//! Entry point: [`structure`].

use tree_sitter::Node;

use super::intelligence::{node_text, span_between, span_trimmed};
use super::types::*;
use super::walk::{Descend, walk_bounded, warn_if_truncated};

/// Every function, prototype, named aggregate, typedef and macro in `root`,
/// in source order.
pub(super) fn structure(root: &Node<'_>, source: &str) -> Vec<StructureItem> {
    let mut items = Vec::new();
    let truncated = walk_bounded(root, |node, _depth| {
        match node.kind() {
            "translation_unit"
            | "preproc_ifdef"
            | "preproc_if"
            | "preproc_elif"
            | "preproc_elifdef"
            | "preproc_else"
            | "linkage_specification"
            | "declaration_list"
            | "ERROR" => return Descend::Children,
            "function_definition" => {
                if let Some(name) = node.child_by_field_name("declarator").and_then(innermost_identifier) {
                    items.push(item(StructureKind::Function, &name, node, node.end_byte(), source));
                }
            }
            "declaration" => {
                let mut cursor = node.walk();
                for declarator in node.children_by_field_name("declarator", &mut cursor) {
                    if let Some(name) = prototype_name(&declarator) {
                        items.push(item(StructureKind::Function, &name, node, node.end_byte(), source));
                    }
                }
                items.extend(defined_aggregate(node, source));
            }
            "type_definition" => {
                let mut cursor = node.walk();
                for declarator in node.children_by_field_name("declarator", &mut cursor) {
                    if let Some(name) = innermost_identifier(declarator) {
                        items.push(item(
                            StructureKind::Other("Type".to_string()),
                            &name,
                            node,
                            node.end_byte(),
                            source,
                        ));
                    }
                }
                items.extend(defined_aggregate(node, source));
            }
            "struct_specifier" | "union_specifier" | "enum_specifier" => {
                items.extend(aggregate(node, source));
            }
            "preproc_def" | "preproc_function_def" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let label = if node.kind() == "preproc_def" {
                        "Constant"
                    } else {
                        "Macro"
                    };
                    items.push(item(
                        StructureKind::Other(label.to_string()),
                        &name,
                        node,
                        node.end_byte(),
                        source,
                    ));
                }
            }
            _ => {}
        }
        Descend::Skip
    });
    warn_if_truncated(truncated, "intel::c", "c");
    items.sort_by_key(|item| item.span.start_byte);
    items
}

/// The identifier at the bottom of a declarator chain, whatever pointers,
/// arrays, parentheses and initialisers wrap it.
///
/// The chain is one grammar node per wrapper, so it is walked with an explicit
/// stack rather than by recursion: a declarator behind 100,000 pointers is a
/// valid parse, and a frame per pointer would overflow the 2 MiB stacks the
/// library runs on. The search is depth first in source order, first match
/// wins, which is the order the recursive form visited.
fn innermost_identifier<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut pending = vec![node];
    while let Some(current) = pending.pop() {
        match current.kind() {
            "identifier" | "type_identifier" | "field_identifier" => return Some(current),
            _ => {
                // ~keep A parenthesised declarator (`(*name)(int)`) holds its inner
                // ~keep declarator as an unnamed child rather than a field.
                if let Some(inner) = current.child_by_field_name("declarator") {
                    pending.push(inner);
                    continue;
                }
                let mut cursor = current.walk();
                let children: Vec<Node<'tree>> = current
                    .named_children(&mut cursor)
                    .filter(|child| child.kind().ends_with("declarator") || child.kind().ends_with("identifier"))
                    .collect();
                pending.extend(children.into_iter().rev());
            }
        }
    }
    None
}

/// The name a prototype declares: pointers may wrap the function declarator,
/// but the declarator itself must apply to a bare identifier.
///
/// Iterative for the same reason as [`innermost_identifier`].
fn prototype_name<'tree>(declarator: &Node<'tree>) -> Option<Node<'tree>> {
    let mut current = *declarator;
    loop {
        match current.kind() {
            "pointer_declarator" => current = current.child_by_field_name("declarator")?,
            "function_declarator" => {
                return current
                    .child_by_field_name("declarator")
                    .filter(|inner| inner.kind() == "identifier");
            }
            _ => return None,
        }
    }
}

/// A named aggregate defined in the `type` field of a typedef or declaration.
fn defined_aggregate(node: &Node<'_>, source: &str) -> Option<StructureItem> {
    let specifier = node.child_by_field_name("type")?;
    aggregate(&specifier, source)
}

/// A `struct`, `union` or `enum` specifier that both names and defines its
/// type. A top-level specifier's terminator is its sibling, and the item
/// runs through it.
fn aggregate(node: &Node<'_>, source: &str) -> Option<StructureItem> {
    let kind = match node.kind() {
        "struct_specifier" => StructureKind::Struct,
        "union_specifier" => StructureKind::Other("Union".to_string()),
        "enum_specifier" => StructureKind::Enum,
        _ => return None,
    };
    let name = node.child_by_field_name("name")?;
    node.child_by_field_name("body")?;
    let end = node
        .next_sibling()
        .filter(|sibling| sibling.kind() == ";")
        .map_or(node.end_byte(), |terminator| terminator.end_byte());
    Some(item(kind, &name, node, end, source))
}

fn item(kind: StructureKind, name: &Node<'_>, node: &Node<'_>, end: usize, source: &str) -> StructureItem {
    let span = if end == node.end_byte() {
        span_trimmed(node, source)
    } else {
        span_between(source, node.start_byte(), end)
    };
    StructureItem {
        kind,
        name: Some(node_text(name, source).to_string()),
        span,
        ..StructureItem::default()
    }
}
