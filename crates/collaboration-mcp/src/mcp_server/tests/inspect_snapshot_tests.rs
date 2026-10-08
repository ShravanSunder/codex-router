use super::*;

#[test]
fn inspect_tool_exposes_native_rejection_message() {
    let message = "native thread is unreadable: fixture refusal";
    let rejection: Result<collaboration_protocol::NativeInspectResult, _> = Err(
        collaboration_service::collaboration_application::NativeSessionFailure::NativeRejected {
            stage: collaboration_service::collaboration_application::NativeSessionStage::Inspect,
            message: message.to_owned(),
            reason: "unknown",
            next_action: "inspectTarget",
            native_code: Some(-32600),
        },
    );

    let result = super::super::application_result(rejection, OperationEffect::None);

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured rejection");
    assert_eq!(structured["message"], message);
    assert_eq!(structured["data"]["message"], message);
    assert_eq!(structured["serviceKind"], "nativeRejected");
    assert_eq!(structured["effect"], "none");
}

#[tokio::test]
async fn inspect_without_a_codex_backend_is_a_typed_unavailable_error() {
    let server = CollaborationMcpServer::catalog_only();
    let target: collaboration_protocol::SessionRef =
        serde_json::from_value(fixture_session("codex-local", "requested-thread")).expect("target");

    let result = server
        .session_inspect(super::super::Parameters(
            collaboration_protocol::NativeInspectParams { target },
        ))
        .await;

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured rejection");
    assert_eq!(structured["kind"], "rejected");
    assert_eq!(structured["effect"], "none");
}

#[test]
fn success_schema_branches_match_main_golden_snapshot() {
    // Set UPDATE_MAIN_SUCCESS_SCHEMA_SNAPSHOT=1 to refresh from the resolved tool catalog.
    let expected: std::collections::BTreeMap<String, Value> =
        serde_json::from_str(include_str!("../snapshots/main_success_schemas.json"))
            .expect("main success schema snapshot");
    let server = CollaborationMcpServer::catalog_only();
    let actual = server
        .resolved_tools()
        .into_iter()
        .filter_map(|tool| {
            tool.output_schema.map(|schema| {
                let union = Value::Object((*schema).clone());
                let reference = union["anyOf"][0]["$ref"]
                    .as_str()
                    .expect("success branch reference");
                let definition_name = reference
                    .strip_prefix("#/$defs/")
                    .expect("local success definition");
                let mut success = union["$defs"][definition_name]
                    .as_object()
                    .expect("success definition object")
                    .clone();
                success.insert("$schema".to_owned(), union["$schema"].clone());
                success.insert(
                    "title".to_owned(),
                    Value::String(definition_name.to_owned()),
                );
                success
                    .entry("type".to_owned())
                    .or_insert_with(|| Value::String("object".to_owned()));
                let definitions = union["$defs"]
                    .as_object()
                    .expect("union definitions")
                    .iter()
                    .filter(|(name, _)| {
                        name.as_str() != definition_name && name.as_str() != "McpToolError"
                    })
                    .map(|(name, definition)| (name.clone(), definition.clone()))
                    .collect::<serde_json::Map<_, _>>();
                if !definitions.is_empty() {
                    success.insert("$defs".to_owned(), Value::Object(definitions));
                }
                (tool.name.to_string(), Value::Object(success))
            })
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    if std::env::var_os("UPDATE_MAIN_SUCCESS_SCHEMA_SNAPSHOT").is_some() {
        let entries = actual
            .iter()
            .enumerate()
            .map(|(index, (name, schema))| {
                let comma = if index + 1 < actual.len() { "," } else { "" };
                format!(
                    "{}:{}{}",
                    serde_json::to_string(name).expect("tool name JSON"),
                    serde_json::to_string(schema).expect("tool schema JSON"),
                    comma
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let snapshot = format!("{{\n{entries}\n}}\n");
        let snapshot_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/mcp_server/snapshots/main_success_schemas.json");
        std::fs::write(snapshot_path, snapshot).expect("write resolved MCP success schemas");
        return;
    }
    assert_eq!(actual.len(), 107);
    assert_eq!(expected.len(), 107);
    for (name, success) in actual {
        assert_eq!(success, expected[&name], "{name} success schema drifted");
    }
}
