use codex_acp_adapter::{AcpSchemaCatalog, McpConfiguration};
use serde_json::json;

#[test]
fn stdio_mcp_configuration_preserves_arguments_and_compares_environment_by_name() {
    let mut schema = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("schema: {error}"));
    let server = json!({"name":"notes.server","command":"/usr/bin/example","args":["second","first"],"env":[{"name":"B","value":"2"},{"name":"A","value":"1"}]});
    let first = McpConfiguration::parse(&mut schema, std::slice::from_ref(&server))
        .unwrap_or_else(|error| panic!("parse: {error}"));
    let mut reordered = server;
    reordered["env"] = json!([{"name":"A","value":"1"},{"name":"B","value":"2"}]);
    let second = McpConfiguration::parse(&mut schema, &[reordered])
        .unwrap_or_else(|error| panic!("parse reordered: {error}"));
    assert!(first == second);
    let overrides = first
        .native_overrides()
        .unwrap_or_else(|| panic!("overrides"));
    assert_eq!(
        overrides["mcp_servers"]["notes.server"]["args"],
        json!(["second", "first"])
    );
    assert_eq!(overrides["mcp_servers"]["notes.server"]["env"]["A"], "1");
    assert!(
        McpConfiguration::parse(&mut schema, &[])
            .unwrap_or_else(|error| panic!("empty: {error}"))
            .native_overrides()
            .is_none()
    );
}

#[test]
fn duplicate_names_and_unsupported_transports_fail_without_dropping_requested_servers() {
    let mut schema = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("schema: {error}"));
    let server = json!({"name":"notes","command":"/usr/bin/example","args":[],"env":[]});
    assert!(McpConfiguration::parse(&mut schema, &[server.clone(), server.clone()]).is_err());
    let mut duplicate_env = server.clone();
    duplicate_env["env"] = json!([{"name":"A","value":"one"},{"name":"A","value":"two"}]);
    assert!(McpConfiguration::parse(&mut schema, &[duplicate_env]).is_err());
    assert!(
        McpConfiguration::parse(
            &mut schema,
            &[json!({"type":"http","name":"remote","url":"https://example.invalid","headers":[]})]
        )
        .is_err()
    );
    let mut relative = server;
    relative["command"] = json!("relative-command");
    assert!(McpConfiguration::parse(&mut schema, &[relative]).is_err());
}
