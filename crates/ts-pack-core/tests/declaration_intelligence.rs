//! Regression coverage for declarations whose grammar shape differs from its name.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use tree_sitter_language_pack::{ProcessConfig, ProcessResult, StructureKind, SymbolKind, process};

fn extract(source: &str, language: &str) -> ProcessResult {
    let result = process(source, &ProcessConfig::new(language).all())
        .expect("real grammar required; build with TSLP_LANGUAGES=swift,typescript,tsx,javascript");
    assert_eq!(result.metrics.error_count, 0, "fixture must parse without recovery");
    result
}

#[test]
fn swift_protocols_have_names_kinds_spans_and_nested_requirements() {
    let source = "public protocol SigningKey: Sendable {\n  func sign()\n}\npublic protocol KeyStorage: Sendable {}\nprotocol Callable: AnyObject {}\n";
    let result = extract(source, "swift");
    let structures: Vec<_> = result
        .structure
        .iter()
        .map(|item| {
            (
                item.name.as_deref(),
                &item.kind,
                item.span.start_line,
                item.span.end_line,
            )
        })
        .collect();
    assert_eq!(
        structures,
        vec![
            (Some("SigningKey"), &StructureKind::Interface, 0, 2),
            (Some("KeyStorage"), &StructureKind::Interface, 3, 3),
            (Some("Callable"), &StructureKind::Interface, 4, 4),
        ]
    );
    assert_eq!(result.structure[0].children[0].name.as_deref(), Some("sign"));
    assert_eq!(result.structure[0].children[0].span.start_line, 1);
    assert_eq!(result.structure[0].children[0].span.end_line, 1);
    for (name, start, end) in [("SigningKey", 0, 2), ("KeyStorage", 3, 3), ("Callable", 4, 4)] {
        let symbol = result.symbols.iter().find(|symbol| symbol.name == name).unwrap();
        assert_eq!(symbol.kind, SymbolKind::Interface);
        assert_eq!((symbol.span.start_line, symbol.span.end_line), (start, end));
    }
}

#[test]
fn swift_nominal_declarations_preserve_struct_actor_and_extension_identity() {
    let source = "struct Keys {\n  func lookup() {}\n}\nactor Store {}\nextension Keys {\n  func fetch() {}\n}\n";
    let result = extract(source, "swift");
    let structures: Vec<_> = result
        .structure
        .iter()
        .map(|item| {
            (
                item.name.as_deref(),
                &item.kind,
                item.span.start_line,
                item.span.end_line,
            )
        })
        .collect();
    assert_eq!(
        structures,
        vec![
            (Some("Keys"), &StructureKind::Struct, 0, 2),
            (Some("Store"), &StructureKind::Class, 3, 3),
            (Some("Keys"), &StructureKind::Impl, 4, 6),
        ]
    );
    assert_eq!(result.structure[2].children[0].name.as_deref(), Some("fetch"));
}

#[test]
fn swift_aliases_and_constants_are_symbols_without_function_local_bindings() {
    let source = "typealias KeyID = String\nlet retryLimit: Int = 3\nstruct Keys {\n  static let shared = 1\n  func lookup() {\n    let local = 2\n  }\n}\n";
    let result = extract(source, "swift");
    let symbols: Vec<_> = result
        .symbols
        .iter()
        .filter(|symbol| matches!(symbol.kind, SymbolKind::Type | SymbolKind::Constant))
        .map(|symbol| {
            (
                symbol.name.as_str(),
                &symbol.kind,
                symbol.span.start_line,
                symbol.span.end_line,
            )
        })
        .collect();
    assert_eq!(
        symbols,
        vec![
            ("KeyID", &SymbolKind::Type, 0, 0),
            ("retryLimit", &SymbolKind::Constant, 1, 1),
            ("Keys", &SymbolKind::Type, 2, 7),
            ("shared", &SymbolKind::Constant, 3, 3),
        ]
    );
    assert!(!result.symbols.iter().any(|symbol| symbol.name == "local"));
}

#[test]
fn typescript_module_constants_and_aliases_exclude_local_variables_and_keep_each_binding() {
    let source = "export type KeyID = string;\nexport const LIMIT: number = 3, SECOND = 4;\nconst CONFIG = { retries: 2 };\nfunction run() {\n  const local = 1;\n  return local;\n}\n";
    let result = extract(source, "typescript");
    let symbols: Vec<_> = result
        .symbols
        .iter()
        .filter(|symbol| matches!(symbol.kind, SymbolKind::Type | SymbolKind::Constant))
        .map(|symbol| {
            (
                symbol.name.as_str(),
                &symbol.kind,
                symbol.span.start_line,
                symbol.span.end_line,
            )
        })
        .collect();
    assert_eq!(
        symbols,
        vec![
            ("KeyID", &SymbolKind::Type, 0, 0),
            ("LIMIT", &SymbolKind::Constant, 1, 1),
            ("SECOND", &SymbolKind::Constant, 1, 1),
            ("CONFIG", &SymbolKind::Constant, 2, 2),
        ]
    );
    assert!(!result.symbols.iter().any(|symbol| symbol.name == "local"));
    for (name, declaration) in [
        ("LIMIT", "LIMIT: number = 3"),
        ("SECOND", "SECOND = 4"),
        ("CONFIG", "CONFIG = { retries: 2 }"),
    ] {
        let symbol = result.symbols.iter().find(|symbol| symbol.name == name).unwrap();
        assert_eq!(&source[symbol.span.start_byte..symbol.span.end_byte], declaration);
    }
}

#[test]
fn named_arrow_bindings_keep_identity_inside_components_and_callbacks_stay_anonymous() {
    let source = "export function Panel() {\n  const loadItems = async () => {\n    return 1;\n  };\n  consume(async () => {\n    await loadItems();\n  });\n  const select = value => value;\n  return <div />;\n}\n";
    let result = extract(source, "tsx");
    let component = &result.structure[0];
    let children: Vec<_> = component
        .children
        .iter()
        .map(|item| {
            (
                item.name.as_deref(),
                &item.kind,
                item.span.start_line,
                item.span.end_line,
            )
        })
        .collect();
    assert_eq!(
        children,
        vec![
            (Some("loadItems"), &StructureKind::Function, 1, 3),
            (None, &StructureKind::Function, 4, 6),
            (Some("select"), &StructureKind::Function, 7, 7),
        ]
    );
}

#[test]
fn destructuring_patterns_do_not_become_literal_containing_symbol_names() {
    let source = "const KEPT = 1;\nconst { limit = 7 } = config;\nconst [first = 9] = values;\n";
    let result = extract(source, "typescript");
    let symbols: Vec<_> = result
        .symbols
        .iter()
        .map(|symbol| (symbol.name.as_str(), &symbol.kind))
        .collect();
    assert_eq!(symbols, vec![("KEPT", &SymbolKind::Constant)]);
}

#[test]
fn arrow_bindings_are_functions_without_constant_duplicates_or_parameter_names() {
    let source = "const convert = item => item;\nconst wrapped = (() => 1);\nconsume(value => value);\n";
    let result = extract(source, "typescript");
    let names: Vec<_> = result.structure.iter().map(|item| item.name.as_deref()).collect();
    assert_eq!(names, vec![Some("convert"), Some("wrapped"), None]);
    assert!(
        result.symbols.is_empty(),
        "function-valued bindings must not also emit constant symbols"
    );
}

#[test]
fn typescript_namespace_supplies_the_exact_owner_span_for_alias_symbols() {
    let source = "namespace Domain {\n  export type ID = string; export type State = number;\n}\nexport const select = value => value;\n";
    let result = extract(source, "typescript");
    let structures: Vec<_> = result
        .structure
        .iter()
        .map(|item| {
            (
                item.name.as_deref(),
                &item.kind,
                item.span.start_line,
                item.span.end_line,
            )
        })
        .collect();
    assert_eq!(
        structures,
        vec![
            (Some("Domain"), &StructureKind::Namespace, 0, 2),
            (Some("select"), &StructureKind::Function, 3, 3),
        ]
    );
    let owner = &result.structure[0].span;
    let aliases: Vec<_> = result
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Type)
        .collect();
    assert_eq!(
        aliases.iter().map(|symbol| symbol.name.as_str()).collect::<Vec<_>>(),
        vec!["ID", "State"]
    );
    for alias in aliases {
        assert!(alias.span.start_byte > owner.start_byte && alias.span.end_byte < owner.end_byte);
        assert_eq!((alias.span.start_line, alias.span.end_line), (1, 1));
    }
}

#[test]
fn generator_bodies_are_not_declaration_scope() {
    let source = "const KEPT = 5;\nfunction* gen() {\n  const local = 1;\n  yield local;\n}\nasync function* agen() {\n  const inner = 2;\n  yield inner;\n}\nconst plain = function* () {\n  const expr = 3;\n  yield expr;\n};\nconst asyncPlain = async function* () {\n  const aexpr = 4;\n  yield aexpr;\n};\nfunction control() {\n  const ordinary = 6;\n  return ordinary;\n}\n";
    let result = extract(source, "javascript");
    let constants: Vec<_> = result
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Constant)
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert_eq!(constants, vec!["KEPT"]);
}
