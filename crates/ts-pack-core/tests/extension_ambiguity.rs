use tree_sitter_language_pack::{detect_language_from_extension, extension_ambiguity};

#[test]
fn should_return_assigned_language_and_all_alternatives_for_ambiguous_extensions() {
    let cases: &[(&str, &str, &[&str])] = &[
        ("h", "c", &["cpp", "objc"]),
        ("gd", "gdscript", &["gap"]),
        ("mod", "gomod", &["fortran"]),
        ("conf", "nginx", &["hocon"]),
        ("m", "objc", &["matlab"]),
        ("pl", "perl", &["prolog"]),
        ("sv", "systemverilog", &["verilog"]),
        ("svh", "systemverilog", &["verilog"]),
        ("v", "v", &["verilog"]),
    ];

    for &(extension, assigned, alternatives) in cases {
        assert_eq!(
            extension_ambiguity(extension),
            Some((assigned, alternatives)),
            "ambiguity for {extension}"
        );
        assert_eq!(detect_language_from_extension(extension), Some(assigned));
    }
}

#[test]
fn should_match_ambiguous_extensions_case_insensitively() {
    for extension in ["conf", "CONF", "CoNf"] {
        assert_eq!(extension_ambiguity(extension), Some(("nginx", &["hocon"][..])));
    }
}

#[test]
fn should_return_none_for_unambiguous_unknown_or_invalid_extensions() {
    for extension in ["py", "rs", "xyz", "", ".m", "file.m", " m", "m ", "m\0", "м", "Ｍ"] {
        assert_eq!(extension_ambiguity(extension), None, "input {extension:?}");
    }
    for length in [32, 33, 4096] {
        assert_eq!(extension_ambiguity(&"m".repeat(length)), None);
    }
}

#[cfg(feature = "serde")]
#[test]
fn should_serialize_assigned_language_and_alternatives_with_historical_field_names() {
    use tree_sitter_language_pack::extension_ambiguity_json;

    assert_eq!(
        extension_ambiguity_json("H"),
        Some(serde_json::json!({"assigned": "c", "alternatives": ["cpp", "objc"]}).to_string())
    );
    for extension in ["py", "xyz", "", ".h", "н"] {
        assert_eq!(extension_ambiguity_json(extension), None, "input {extension:?}");
    }
}
