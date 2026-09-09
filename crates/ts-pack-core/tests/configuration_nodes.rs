use tree_sitter_language_pack::{DataNode, DataNodeKind, ProcessConfig, process};

fn data(source: &str, language: &str) -> DataNode {
    process(source, &ProcessConfig::new(language).with_data_extraction(true))
        .expect("valid configuration must parse")
        .data
        .expect("supported configuration must expose data")
}

#[test]
fn yaml_flow_sequences_preserve_item_keys_and_spans() {
    let source = "services: [{name: first}, {port: 80}]\n";
    let root = data(source, "yaml");
    let services = &root.children[0];
    assert_eq!(services.key.as_deref(), Some("services"));
    assert_eq!(services.children.len(), 2);
    for (index, item) in services.children.iter().enumerate() {
        assert_eq!(item.key.as_deref(), Some(index.to_string().as_str()));
        assert_eq!(item.children.len(), 1);
        assert_eq!(item.span.start_line, 0);
        assert_eq!(item.span.end_line, 0);
    }
    assert_eq!(services.children[0].children[0].key.as_deref(), Some("name"));
    assert_eq!(services.children[1].children[0].key.as_deref(), Some("port"));
}

#[test]
fn yaml_flow_sequence_comments_do_not_change_ordinals() {
    let root = data(
        "services: [\n  # comment\n  {name: first},\n  # another comment\n  {name: second}\n]\n",
        "yaml",
    );
    let items = &root.children[0].children;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].key.as_deref(), Some("0"));
    assert_eq!(items[1].key.as_deref(), Some("1"));
}

#[test]
fn yaml_structured_keys_do_not_become_scalar_paths() {
    let root = data("? {nested: VALUE_SENTINEL}\n: ignored\nsafe: true\n", "yaml");
    let keys: Vec<_> = root.children.iter().filter_map(|node| node.key.as_deref()).collect();
    assert_eq!(keys, ["safe"]);
}

#[test]
fn toml_dotted_keys_follow_segments_instead_of_source_spacing() {
    let root = data(
        "service . \"connection\" = { retry . count = 3 }\n[profile . \"release\"]\nopt-level = 3\n",
        "toml",
    );
    assert_eq!(root.children[0].key.as_deref(), Some("service.connection"));
    assert_eq!(root.children[0].children[0].key.as_deref(), Some("retry.count"));
    assert_eq!(root.children[1].key.as_deref(), Some("profile.release"));
}

#[test]
fn toml_array_comments_do_not_change_ordinals() {
    let root = data(
        "items = [\n {name = \"first\"},\n # comment\n {name = \"second\"}\n]\n",
        "toml",
    );
    let items = &root.children[0].children;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].key.as_deref(), Some("0"));
    assert_eq!(items[1].key.as_deref(), Some("1"));
    assert_eq!(items[1].span.start_line, 3);
}

#[test]
fn json_property_span_includes_the_key_before_a_multiline_value() {
    let source = "{\n \"properties\":\n {\"enabled\": true}\n}\n";
    let root = data(source, "json");
    let property = &root.children[0];
    assert_eq!(property.key.as_deref(), Some("properties"));
    assert_eq!(property.span.start_line, 1);
    assert_eq!(
        &source[property.span.start_byte..property.span.end_byte],
        "\"properties\":\n {\"enabled\": true}"
    );
}

#[test]
fn terraform_uses_the_generic_hcl_data_adapter() {
    let source = "resource \"aws_ecs_service\" \"app\" {\n  desired_count = 1\n}\n";
    let hcl = data(source, "hcl");
    let terraform = data(source, "terraform");
    assert_eq!(terraform.children.len(), 1);
    assert_eq!(terraform.children[0].key, hcl.children[0].key);
    let span = &terraform.children[0].span;
    assert_eq!((span.start_line, span.end_line), (0, 2));
    assert_eq!(&source[span.start_byte..span.end_byte], source.trim_end());
}

#[test]
fn hcl_block_labels_are_key_segments_without_quotes() {
    let root = data("resource \"aws_ecs_service\" \"app\" {}\n", "hcl");
    assert_eq!(root.children[0].key.as_deref(), Some("resource.aws_ecs_service.app"));
}

#[test]
fn toml_trailing_comments_preserve_container_children_and_spans() {
    let source = "service = {retry = 3} # note\nitems = [\n {name = \"first\"},\n {name = \"second\"}\n] # note\n";
    let root = data(source, "toml");
    assert_eq!(root.children[0].children.len(), 1);
    assert_eq!(root.children[0].children[0].key.as_deref(), Some("retry"));
    let items = &root.children[1];
    assert_eq!(items.children.len(), 2);
    assert_eq!((items.span.start_line, items.span.end_line), (1, 4));
    assert_eq!(items.children[1].children[0].key.as_deref(), Some("name"));
}

#[test]
fn yaml_shorthand_flow_mappings_preserve_ordinals_and_keys() {
    let root = data("items: [foo: bar, baz: quux]\n", "yaml");
    let items = &root.children[0].children;
    assert_eq!(items.len(), 2);
    for (index, key) in ["foo", "baz"].iter().enumerate() {
        assert_eq!(items[index].key.as_deref(), Some(index.to_string().as_str()));
        assert_eq!(items[index].children[0].key.as_deref(), Some(*key));
    }
}

#[test]
fn json_multiple_roots_do_not_return_only_the_first() {
    assert_eq!(data("{\"a\":1}\n", "json").children[0].key.as_deref(), Some("a"));
    let result = process(
        "{\"a\":1}\n{\"b\":2}\n",
        &ProcessConfig::new("json").with_data_extraction(true),
    )
    .expect("the grammar accepts multiple roots");
    assert_eq!(result.metrics.error_count, 0);
    assert!(
        result.data.is_none(),
        "multiple roots must not yield a partial data tree"
    );
}

#[test]
fn plist_dict_entries_are_keyed_by_their_key_text_and_arrays_by_position() {
    let source = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<array>
  <dict>
    <key>BundleIsRelocatable</key>
    <false/>
    <key>BundleOverwriteAction</key>
    <string>upgrade</string>
    <key>Paths</key>
    <array>
      <string>one</string>
      <string>two</string>
    </array>
  </dict>
</array>
</plist>
";
    let root = data(source, "xml");
    assert_eq!(root.children.len(), 1);
    let item = &root.children[0];
    assert_eq!(item.key.as_deref(), Some("0"));
    assert_eq!(item.kind, DataNodeKind::Sequence);
    assert_eq!(item.value, None);
    assert_eq!((item.span.start_line, item.span.end_line), (4, 14));
    let entries: Vec<(Option<&str>, Option<&str>, usize, usize)> = item
        .children
        .iter()
        .map(|entry| {
            (
                entry.key.as_deref(),
                entry.value.as_deref(),
                entry.span.start_line,
                entry.span.end_line,
            )
        })
        .collect();
    assert_eq!(
        entries,
        vec![
            (Some("BundleIsRelocatable"), Some("false"), 5, 6),
            (Some("BundleOverwriteAction"), Some("upgrade"), 7, 8),
            (Some("Paths"), None, 9, 13),
        ]
    );
    let paths = &item.children[2].children;
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[1].key.as_deref(), Some("1"));
    assert_eq!(paths[1].value.as_deref(), Some("two"));
}

#[test]
fn xml_that_is_not_a_plist_keeps_its_element_paths() {
    let root = data("<plist-like><key>Name</key></plist-like>\n", "xml");
    assert_eq!(root.children[0].key.as_deref(), Some("plist-like"));
    assert_eq!(root.children[0].children[0].key.as_deref(), Some("key"));
}

#[test]
fn dotenv_assignments_are_keyed_scalars_with_values_as_written() {
    let source = "\
# comment KEY=IGNORED
PLAIN=value
export EXPORTED=1
EMPTY=
PLACEHOLDER=${OTHER}
MULTI=\"line one
line two\"
DUP=1
DUP=2
";
    let root = data(source, "dotenv");
    let entries: Vec<(Option<&str>, Option<&str>, usize, usize)> = root
        .children
        .iter()
        .map(|entry| {
            (
                entry.key.as_deref(),
                entry.value.as_deref(),
                entry.span.start_line,
                entry.span.end_line,
            )
        })
        .collect();
    assert_eq!(
        entries,
        vec![
            (Some("PLAIN"), Some("value"), 1, 1),
            (Some("EXPORTED"), Some("1"), 2, 2),
            (Some("EMPTY"), Some(""), 3, 3),
            (Some("PLACEHOLDER"), Some("${OTHER}"), 4, 4),
            (Some("MULTI"), Some("\"line one\nline two\""), 5, 6),
            (Some("DUP"), Some("1"), 7, 7),
            (Some("DUP"), Some("2"), 8, 8),
        ]
    );
}
