use super::CollaborationMcpServer;
use serde_json::Value;
use std::collections::BTreeSet;

#[test]
fn message_adapter_preserves_preparation_and_submission_effects() {
    let transport = || {
        collaboration_client::ClientError::Transport(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "fixture",
        ))
    };
    let preparation = super::message_tool_result(Err(
        collaboration_client::MessageSendError::Preparation(transport()),
    ));
    assert_eq!(preparation.is_error, Some(true));
    assert_eq!(
        preparation
            .structured_content
            .as_ref()
            .and_then(|value| value.get("effect")),
        Some(&serde_json::json!("none"))
    );
    let submission = super::message_tool_result(Err(
        collaboration_client::MessageSendError::Submission(transport()),
    ));
    assert_eq!(submission.is_error, Some(true));
    assert_eq!(
        submission
            .structured_content
            .as_ref()
            .and_then(|value| value.get("effect")),
        Some(&serde_json::json!("unknown"))
    );
}

#[test]
fn catalog_has_complete_unique_tools_with_resolvable_schemas() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let tools = server.resolved_tools();
    assert_eq!(tools.len(), 89);
    let mut names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 89);
    for tool in tools {
        let input = serde_json::to_value(&tool.input_schema).expect("input schema JSON");
        jsonschema::validator_for(&input)
            .unwrap_or_else(|error| panic!("{} input schema: {error}", tool.name));
        let output = serde_json::to_value(
            tool.output_schema
                .as_ref()
                .unwrap_or_else(|| panic!("{} output schema", tool.name)),
        )
        .expect("output schema JSON");
        jsonschema::validator_for(&output)
            .unwrap_or_else(|error| panic!("{} output schema: {error}", tool.name));
    }
}

#[test]
fn typed_tool_names_cover_every_control_domain_operation() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let actual = server
        .tool_router
        .list_all()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<BTreeSet<_>>();
    let document =
        collaboration_protocol::control_schema_document(None).expect("Control schema document");
    let methods = document
        .get("x-methods")
        .and_then(Value::as_object)
        .expect("Control method map");
    let expected = methods
        .keys()
        .filter(|method| method.as_str() != "control/initialize")
        .map(|method| expected_tool_name(method))
        .chain([
            "conversation_create".to_owned(),
            "conversation_prompt".to_owned(),
            "events_observe".to_owned(),
        ])
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn sdk_only_operations_are_advertised_by_both_cli_and_mcp_catalogs() {
    let help = agent_collaboration::command_help();
    for (tool, cli_command) in [
        ("conversation_create", "conversation create"),
        ("conversation_prompt", "conversation prompt"),
        ("events_observe", "events observe"),
    ] {
        assert!(help.contains(cli_command), "CLI adapter missing for {tool}");
    }
}

#[test]
fn native_schema_refs_bind_when_advertised_and_remain_honestly_opaque_otherwise() {
    let source = serde_json::json!({"properties":{"thread":{"$ref":"urn:codex-native:Thread"}}});
    let mut unavailable = source.clone();
    super::bind_native_schema_refs(&mut unavailable, None);
    assert_eq!(
        unavailable.pointer("/properties/thread"),
        Some(&serde_json::json!({}))
    );

    let definitions = serde_json::Map::from_iter([(
        "Thread".to_owned(),
        serde_json::json!({"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}),
    )]);
    let mut available = source;
    super::bind_native_schema_refs(&mut available, Some(&definitions));
    assert_eq!(
        available.pointer("/properties/thread/$ref"),
        Some(&serde_json::json!("#/$defs/codexNativeThread"))
    );
    jsonschema::validator_for(&available).expect("bound native schema compiles");
}

#[test]
fn advertised_native_bundle_is_loaded_into_discovered_tool_schema() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let control_hex = "a".repeat(64);
    let native_hex = "b".repeat(64);
    std::fs::write(
        temporary.path().join("service.json"),
        serde_json::to_vec(&serde_json::json!({
            "controlSchemaDigest": format!("sha256:{control_hex}")
        }))
        .expect("manifest JSON"),
    )
    .expect("manifest write");
    std::fs::write(
        temporary
            .path()
            .join(format!("control-schema-{control_hex}.json")),
        serde_json::to_vec(&serde_json::json!({
            "x-nativeSchemaDigest": format!("sha256:{native_hex}")
        }))
        .expect("Control schema JSON"),
    )
    .expect("Control schema write");
    std::fs::write(
        temporary.path().join(format!("{native_hex}.json")),
        serde_json::to_vec(&serde_json::json!({
            "documents": {
                "codex_app_server_protocol.schemas.json": {
                        "definitions": {"v2": {
                            "AbsolutePathBuf":{"type":"string","minLength":1},
                            "ThreadEnvironment":{"type":"object","required":["cwd"],"properties":{
                                "cwd":{"$ref":"#/definitions/v2/AbsolutePathBuf"}
                            }},
                            "ThreadExtra":{"type":"object","properties":{
                                "parent":{"$ref":"#/definitions/v2/Thread"}
                            }},
                            "Unrelated":{"type":"object","properties":{"ignored":{"type":"boolean"}}},
                            "Thread": {
                                "type":"object","required":["id","environment"],
                                "properties":{
                                    "id":{"type":"string"},
                                    "environment":{"$ref":"#/definitions/v2/ThreadEnvironment"},
                                    "extra":{"$ref":"#/definitions/v2/ThreadExtra"}
                                }
                            }
                        }}
                }
            }
        }))
        .expect("native schema JSON"),
    )
    .expect("native schema write");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let inspect = server
        .resolved_tools()
        .into_iter()
        .find(|tool| tool.name == "session_inspect")
        .expect("session inspect tool");
    let schema = serde_json::Value::Object(
        (**inspect
            .output_schema
            .as_ref()
            .expect("inspect output schema"))
        .clone(),
    );
    let encoded = serde_json::to_string(&schema).expect("schema encoding");
    assert!(encoded.contains("codexNativeThreadEnvironment"));
    assert!(encoded.contains("#/$defs/codexNativeAbsolutePathBuf"));
    assert!(encoded.contains("codexNativeThreadExtra"));
    assert!(!encoded.contains("codexNativeUnrelated"));
    let validator = jsonschema::validator_for(&schema).expect("advertised native schema compiles");
    let result = serde_json::json!({
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"thread"},
        "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},
        "effectiveAccess":null,
        "settingsObservation":{"kind":"unavailable","reason":"threadReadOmitsSettings"},
        "thread":{"id":"thread","environment":{"cwd":"/tmp/project"}}
    });
    assert!(validator.is_valid(&result));

    let board = server
        .resolved_tools()
        .into_iter()
        .find(|tool| tool.name == "board_list")
        .expect("board list tool");
    let board_schema = serde_json::to_string(&board.input_schema).expect("board schema encoding");
    assert!(!board_schema.contains("codexNative"));
}

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
            "control":{"transport":"unixJsonLines","path":"control.sock"},
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

fn expected_tool_name(method: &str) -> String {
    match method {
        "endpoint/list" => return "endpoints_list".to_owned(),
        "codex/sessionList" => return "sessions_list".to_owned(),
        "codex/sessionInspect" => return "session_inspect".to_owned(),
        "codex/sessionRename" => return "session_rename".to_owned(),
        "codex/messageSend" => return "message_send".to_owned(),
        "codex/turnInterrupt" => return "turn_interrupt".to_owned(),
        "lifecycleJournal/status" => return "journal_status".to_owned(),
        "lifecycleJournal/read" => return "journal_read".to_owned(),
        "addressBook/list" => return "addresses_list".to_owned(),
        "wake/subscribe" => return "wake_wait_until_first_fire".to_owned(),
        _ => {}
    }
    let mut output = String::new();
    for character in method.chars() {
        if character == '/' {
            output.push('_');
        } else if character.is_ascii_uppercase() {
            output.push('_');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}
