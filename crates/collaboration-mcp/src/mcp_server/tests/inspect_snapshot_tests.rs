use super::*;

#[tokio::test]
async fn inspect_tool_rejects_well_shaped_wrong_target_response_like_typed_sdk() {
    use rmcp::handler::server::wrapper::Parameters;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let temporary = tempfile::tempdir().expect("temporary directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private directory");
    }
    let digest = format!("sha256:{}", "a".repeat(64));
    std::fs::write(
        temporary.path().join("service.json"),
        serde_json::to_vec(&serde_json::json!({
            "version":2,
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002",
            "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":digest,
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        }))
        .expect("manifest JSON"),
    )
    .expect("manifest write");
    let listener = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control bind");
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("Control accept");
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let initialize: serde_json::Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("read init")
                .expect("init frame"),
        )
        .expect("init JSON");
        let initialized = serde_json::json!({"jsonrpc":"2.0","id":initialize["id"],"result":{
            "version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002","controlSchemaDigest":format!("sha256:{}", "a".repeat(64))
        }});
        write
            .write_all(format!("{initialized}\n").as_bytes())
            .await
            .expect("write init");
        let inspect: serde_json::Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("read inspect")
                .expect("inspect frame"),
        )
        .expect("inspect JSON");
        let wrong_target = serde_json::json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"other-thread"});
        let response = serde_json::json!({"jsonrpc":"2.0","id":inspect["id"],"result":{
            "target":wrong_target,"generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},
            "effectiveAccess":null,"settingsObservation":{"kind":"unavailable","reason":"threadReadOmitsSettings"},"thread":{"id":"other-thread"}
        }});
        write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .expect("write inspect");
    });
    let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"requested-thread"
        }))
        .expect("target");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let result = server
        .session_inspect(Parameters(collaboration_protocol::NativeInspectParams {
            target,
        }))
        .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("kind")),
        Some(&serde_json::json!("protocolViolation"))
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), peer)
        .await
        .expect("Control fixture traffic deadline")
        .expect("peer join");
}

#[tokio::test]
async fn inspect_tool_exposes_native_rejection_message() {
    use rmcp::handler::server::wrapper::Parameters;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let temporary = tempfile::tempdir().expect("temporary directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private directory");
    }
    let digest = format!("sha256:{}", "a".repeat(64));
    std::fs::write(
        temporary.path().join("service.json"),
        serde_json::to_vec(&serde_json::json!({
            "version":2,
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002",
            "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":digest,
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        }))
        .expect("manifest JSON"),
    )
    .expect("manifest write");
    let listener = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control bind");
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("Control accept");
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let initialize: serde_json::Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("read init")
                .expect("init frame"),
        )
        .expect("init JSON");
        let initialized = serde_json::json!({"jsonrpc":"2.0","id":initialize["id"],"result":{
            "version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002","controlSchemaDigest":format!("sha256:{}", "a".repeat(64))
        }});
        write
            .write_all(format!("{initialized}\n").as_bytes())
            .await
            .expect("write init");
        let inspect: serde_json::Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("read inspect")
                .expect("inspect frame"),
        )
        .expect("inspect JSON");
        assert_eq!(inspect["method"], "codex/sessionInspect");
        let message = "native thread is unreadable: fixture refusal";
        let response = serde_json::json!({"jsonrpc":"2.0","id":inspect["id"],"error":{
            "code":-32050,"message":message,"data":{
                "kind":"nativeRejected","stage":"inspect","message":message,
                "reason":"unknown","nextAction":"inspectTarget","nativeCode":-32600
            }
        }});
        write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .expect("write rejection");
    });
    let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"unreadable-thread"
    }))
    .expect("target");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());

    let result = server
        .session_inspect(Parameters(collaboration_protocol::NativeInspectParams {
            target,
        }))
        .await;

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured rejection");
    assert_eq!(
        structured["message"],
        "native thread is unreadable: fixture refusal"
    );
    assert_eq!(
        structured["data"]["message"],
        "native thread is unreadable: fixture refusal"
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), peer)
        .await
        .expect("Control fixture traffic deadline")
        .expect("peer join");
}

#[test]
fn success_schema_branches_match_main_golden_snapshot() {
    // Set UPDATE_MAIN_SUCCESS_SCHEMA_SNAPSHOT=1 to refresh from the resolved tool catalog.
    let expected: std::collections::BTreeMap<String, Value> =
        serde_json::from_str(include_str!("../snapshots/main_success_schemas.json"))
            .expect("main success schema snapshot");
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
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
