use tree_sitter_language_pack::{DataNode, ProcessConfig, process};

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
fn yaml_alias_keys_are_preserved() {
    let source =
        "name: &key value\n*key : retained\nbase: &shape {nested: 1}\n*shape : kept\nlist:\n- &item one\n- *item\n";
    let result =
        process(source, &ProcessConfig::new("yaml").with_data_extraction(true)).expect("alias keys are valid YAML");
    assert_eq!(result.metrics.error_count, 0, "fixture must parse without recovery");
    let root = result.data.expect("yaml exposes data");
    let entries: Vec<_> = root
        .children
        .iter()
        .map(|node| (node.key.as_deref(), node.value.as_deref()))
        .collect();
    assert_eq!(
        entries,
        vec![
            (Some("name"), Some("value")),
            (Some("*key"), Some("retained")),
            (Some("base"), None),
            (Some("*shape"), Some("kept")),
            (Some("list"), None),
        ]
    );
    let items: Vec<_> = root.children[4]
        .children
        .iter()
        .map(|node| node.value.as_deref())
        .collect();
    assert_eq!(items, vec![Some("one"), Some("*item")]);
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

/// Set by the parent test so the child process runs the deep-key probe for real.
const TOML_DEEP_KEY_PROBE_ENV: &str = "TSLP_TOML_DEEP_KEY_PROBE";
const TOML_DEEP_KEY_SEGMENTS: usize = 10_000;
const TOML_DEEP_KEY_SENTINEL: &str = "toml deep dotted key probe completed";

/// The probe itself. Ignored so a plain run never executes it in-process: a stack
/// overflow is an abort, not a panic, and would take every other test in this
/// binary down with it. The parent below re-executes this binary with the env var
/// set and asks for exactly this test.
#[test]
#[ignore]
fn toml_deep_dotted_key_probe() {
    if std::env::var_os(TOML_DEEP_KEY_PROBE_ENV).is_none() {
        return;
    }
    let source = format!("{} = 1\n", vec!["a"; TOML_DEEP_KEY_SEGMENTS].join("."));
    // ~keep 2 MiB is the smallest stack the library runs on (tokio spawn_blocking and the
    // ~keep Node, Python and JVM worker threads); the default test thread is not that small.
    let worker = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || data(&source, "toml"))
        .expect("spawn probe thread");
    let root = worker.join().expect("probe thread must not panic");
    assert_eq!(root.children.len(), 1);
    let key = root.children[0].key.as_deref().expect("dotted key");
    assert_eq!(key.split('.').count(), TOML_DEEP_KEY_SEGMENTS);
    assert!(key.split('.').all(|segment| segment == "a"));
    println!("{TOML_DEEP_KEY_SENTINEL}");
}

/// A valid 10,000-segment dotted key must not abort the process.
///
/// The TOML grammar nests one `dotted_key` node per segment, so a key builder that
/// recurses per segment walks off a 2 MiB stack long before the depth guard the
/// other builders share can see it. An abort cannot be caught in-process, so the
/// probe runs in a child and this test reads its exit status and its sentinel line;
/// the sentinel is what stops a filtered-out or silently skipped probe from passing
/// as a survival.
#[test]
fn toml_deep_dotted_key_survives_a_two_mebibyte_stack() {
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(exe)
        .args(["--ignored", "--exact", "toml_deep_dotted_key_probe", "--nocapture"])
        .env(TOML_DEEP_KEY_PROBE_ENV, "1")
        .output()
        .expect("spawn the probe child process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains(TOML_DEEP_KEY_SENTINEL),
        "the deep dotted key probe did not survive (status {:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
}
