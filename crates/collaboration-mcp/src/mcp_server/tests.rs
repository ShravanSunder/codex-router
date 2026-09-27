use super::{
    CollaborationMcpServer, ConversationCreateToolRequest, conversation_create_tool_result,
};
use collaboration_protocol::{ConversationCreateOutcome, DeliveryReceipt, OperationId};
use serde_json::Value;
use std::collections::BTreeSet;

#[tokio::test]
async fn native_sessions_list_routes_claude_to_provider_tool_without_changing_schema() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let request: collaboration_protocol::NativeSessionListParams = serde_json::from_value(
        serde_json::json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
            "view":"active","scope":{"kind":"any"},"source":"interactive",
            "pageSize":10
        }),
    ).expect("Claude list request");
    let result = server.sessions_list(super::Parameters(request)).await;
    assert_eq!(result.is_error, Some(true));
    let content = serde_json::to_string(&result.structured_content).expect("error content");
    assert!(content.contains("provider_sessions_list"), "{content}");
}

#[test]
fn message_send_route_receipts_match_advertised_output_schema() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let schema = server
        .resolved_tools()
        .into_iter()
        .find(|tool| tool.name == "message_send")
        .and_then(|tool| tool.output_schema)
        .expect("advertised message_send output schema");
    let schema = Value::Object((*schema).clone());
    assert!(schema["anyOf"].is_array(), "tool output must use anyOf");
    assert_eq!(
        schema.pointer("/$defs/McpToolError/oneOf/0/properties/mcpResult/const"),
        Some(&serde_json::json!("error")),
        "error branch must have an exclusive discriminator"
    );
    let validator = jsonschema::validator_for(&schema).expect("message_send JSON Schema");
    let target = serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"thread-a"
    });
    let generation = serde_json::json!({
        "serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1
    });
    let native = |acceptance: Value| {
        serde_json::json!({
            "outcome":{"kind":"startedOrSteered"},
            "reachability":"codexAppServer",
            "client":{
                "kind":"codexAppServer",
                "target":target,
                "generation":generation,
                "inputKind":"agent",
                "representation":"declaredAgentText",
                "clientUserMessageId":"message-a",
                "resumeEffect":"notRequested",
                "acceptance":acceptance
            }
        })
    };
    let cases = [
        (
            "codex-start",
            native(serde_json::json!({
                "kind":"nativeInputAccepted","operation":"turnStart",
                "disposition":"startedOrSteered","turnId":"turn-a"
            })),
        ),
        (
            "codex-steer",
            native(serde_json::json!({
                "kind":"steerAccepted","turnId":"turn-a"
            })),
        ),
        (
            "claude-peer",
            serde_json::json!({
                "outcome":{"kind":"peerMessageWritten"},"reachability":"claudeCodePeer",
                "client":{"kind":"claudeCodePeer"}
            }),
        ),
        (
            "provider-acp",
            serde_json::json!({
                "outcome":{"kind":"started"},"reachability":"providerAcp",
                "client":{"kind":"providerAcp","operationId":OperationId::generate()}
            }),
        ),
    ];
    for (route, value) in cases {
        let receipt: DeliveryReceipt = serde_json::from_value(value.clone())
            .unwrap_or_else(|error| panic!("{route} receipt fixture: {error}"));
        let result = super::message_tool_result(Ok(receipt));
        assert_ne!(result.is_error, Some(true), "{route} tool result");
        let structured = result.structured_content.expect("structured receipt");
        assert_eq!(structured, value, "{route} success wire shape changed");
        validator.validate(&structured).unwrap_or_else(|error| {
            panic!("{route} structuredContent violates advertised schema: {error}; {structured}")
        });
    }
}

#[test]
fn message_send_post_submission_failure_matches_advertised_output_schema() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let schema = server
        .resolved_tools()
        .into_iter()
        .find(|tool| tool.name == "message_send")
        .and_then(|tool| tool.output_schema)
        .expect("advertised message_send output schema");
    let validator = jsonschema::validator_for(&Value::Object((*schema).clone()))
        .expect("message_send JSON Schema");
    let target = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"thread-a"
    }))
    .expect("target");
    let result =
        super::message_tool_result(Err(collaboration_client::MessageSendError::Submission {
            target,
            source: Box::new(collaboration_client::ClientError::Protocol(
                "invalid message receipt; acceptance unknown",
            )),
        }));
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error receipt");
    validator.validate(&structured).unwrap_or_else(|error| {
        panic!(
            "post-submission structuredContent violates advertised schema: {error}; {structured}"
        )
    });
}

#[test]
fn message_send_unknown_delivery_retains_outcome_in_typed_error() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let schema = server
        .resolved_tools()
        .into_iter()
        .find(|tool| tool.name == "message_send")
        .and_then(|tool| tool.output_schema)
        .expect("advertised message_send output schema");
    let schema = Value::Object((*schema).clone());
    let validator = jsonschema::validator_for(&schema).expect("message_send JSON Schema");
    let receipt = DeliveryReceipt {
        outcome: collaboration_protocol::DeliveryOutcome::Unknown,
        reachability: None,
        client: None,
    };
    let result = super::message_tool_result(Ok(receipt));
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error receipt");
    assert_eq!(structured["mcpResult"], "error");
    assert_eq!(structured["kind"], "outcomeUnknown");
    assert_eq!(structured["effect"], "unknown");
    assert_eq!(structured["outcome"]["kind"], "unknown");
    validator.validate(&structured).expect("typed error schema");
}

#[test]
fn message_send_foreign_writer_rejection_has_typed_action_in_mcp_error() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let schema = server
        .resolved_tools()
        .into_iter()
        .find(|tool| tool.name == "message_send")
        .and_then(|tool| tool.output_schema)
        .expect("advertised message_send output schema");
    let validator = jsonschema::validator_for(&Value::Object((*schema).clone()))
        .expect("message_send JSON Schema");
    let receipt: DeliveryReceipt = serde_json::from_value(serde_json::json!({
        "outcome":{
            "kind":"rejected","reason":"heldByAnotherClient",
            "nextAction":"messageFromHoldingCodexClient","clientCode":-32600,
            "detail":"Message it from the Codex client that holds it."
        },
        "reachability":"codexAppServer","client":null
    }))
    .expect("typed delivery receipt");
    let result = super::message_tool_result(Ok(receipt));
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured MCP error");
    assert_eq!(structured["mcpResult"], "error");
    assert_eq!(structured["outcome"]["reason"], "heldByAnotherClient");
    assert_eq!(
        structured["outcome"]["nextAction"],
        "messageFromHoldingCodexClient"
    );
    assert_eq!(
        structured["message"],
        "Message it from the Codex client that holds it."
    );
    validator
        .validate(&structured)
        .expect("typed MCP output schema");
}

#[test]
fn message_adapter_preserves_preparation_and_submission_effects() {
    let transport = || {
        collaboration_client::ClientError::Transport(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "fixture",
        ))
    };
    let preparation = super::message_tool_result(Err(
        collaboration_client::MessageSendError::Preparation(Box::new(transport())),
    ));
    assert_eq!(preparation.is_error, Some(true));
    assert_eq!(
        preparation
            .structured_content
            .as_ref()
            .and_then(|value| value.get("effect")),
        Some(&serde_json::json!("none"))
    );
    let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"message-thread"
    }))
    .expect("target");
    let submission =
        super::message_tool_result(Err(collaboration_client::MessageSendError::Submission {
            target: target.clone(),
            source: Box::new(transport()),
        }));
    assert_eq!(submission.is_error, Some(true));
    assert_eq!(
        submission
            .structured_content
            .as_ref()
            .and_then(|value| value.get("effect")),
        Some(&serde_json::json!("unknown"))
    );
    assert_eq!(
        submission
            .structured_content
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&serde_json::json!(target))
    );

    let rejection =
        super::message_tool_result(Err(collaboration_client::MessageSendError::Preparation(
            Box::new(collaboration_client::ClientError::Rejected {
                code: -32050,
                data: Some(serde_json::json!({
                    "kind":"staleGeneration",
                    "stage":"discovery",
                    "expected":2
                })),
            }),
        )));
    let structured = rejection.structured_content.expect("structured rejection");
    assert_eq!(structured["kind"], "rejected");
    assert_eq!(structured["serviceKind"], "staleGeneration");
    assert_eq!(structured["stage"], "discovery");
    assert_eq!(structured["effect"], "none");
    assert_eq!(structured["code"], -32050);
    assert_eq!(structured["data"]["expected"], 2);
}

#[test]
fn native_control_errors_keep_service_detail_and_only_uncertain_effects_are_unknown() {
    let unsupported = super::structured_result::<serde_json::Value>(
        Err(collaboration_client::ClientError::Rejected {
            code: -32050,
            data: Some(serde_json::json!({
                "kind":"unsupportedCapability",
                "stage":"rename",
                "message":"Codex app-server method `thread/name/set` is missing from its cached schema"
            })),
        }),
        collaboration_protocol::OperationEffect::Unknown,
    );
    let unsupported = unsupported.structured_content.expect("unsupported detail");
    assert_eq!(
        unsupported["message"],
        "Codex app-server method `thread/name/set` is missing from its cached schema"
    );
    assert_eq!(unsupported["effect"], "none");
    assert_eq!(unsupported["data"]["kind"], "unsupportedCapability");

    let lost_after_send = super::structured_result::<serde_json::Value>(
        Err(collaboration_client::ClientError::Protocol(
            "connection closed after dispatch",
        )),
        collaboration_protocol::OperationEffect::Unknown,
    );
    let lost_after_send = lost_after_send
        .structured_content
        .expect("transport detail");
    assert_eq!(lost_after_send["effect"], "unknown");
    assert_eq!(
        lost_after_send["message"],
        "Control protocol violation: connection closed after dispatch"
    );
}

#[test]
fn catalog_has_complete_unique_tools_with_resolvable_schemas() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let tools = server.resolved_tools();
    assert_eq!(tools.len(), 103);
    let mut names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 103);
    assert!(names.contains(&"conversation_resume"));
    assert!(names.contains(&"conversation_close"));
    assert!(names.contains(&"provider_sessions_list"));
    assert!(names.contains(&"provider_session_inspect"));
    assert!(names.contains(&"question_list"));
    assert!(names.contains(&"question_answer"));
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
        .filter(|method| {
            !matches!(
                method.as_str(),
                "control/initialize" | "provider/sessionObserve" | "provider/sessionListen"
            )
        })
        .map(|method| expected_tool_name(method))
        .chain([
            "conversation_create".to_owned(),
            "conversation_create_and_prompt".to_owned(),
            "conversation_prompt".to_owned(),
            "events_observe".to_owned(),
        ])
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
    assert!(
        actual
            .iter()
            .all(|name| !name.starts_with("provider_conversation_")),
        "the MCP catalog must expose one conversation surface"
    );
}

#[test]
fn conversation_catalog_defers_operation_id_requirement_until_route_selection() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let tools = server.tool_router.list_all();
    for name in ["conversation_load", "conversation_prompt"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("common tool");
        let required = tool.input_schema["required"]
            .as_array()
            .expect("required fields");
        assert!(
            !required.contains(&serde_json::json!("operationId")),
            "{name} selects its client before requiring an operation ID"
        );
    }
    let create = tools
        .iter()
        .find(|tool| tool.name == "conversation_create")
        .expect("create tool");
    assert!(
        create.input_schema["required"]
            .as_array()
            .expect("create required fields")
            .contains(&serde_json::json!("operationId"))
    );
    let convenience = tools
        .iter()
        .find(|tool| tool.name == "conversation_create_and_prompt")
        .expect("create and prompt tool");
    assert!(
        !convenience.input_schema["required"]
            .as_array()
            .expect("convenience required fields")
            .contains(&serde_json::json!("promptOperationId"))
    );
}

#[test]
fn tool_schemas_match_known_runtime_defaults_and_conditional_requirements() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let tools = server.resolved_tools();
    let schema_for = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("missing tool {name}"))
            .input_schema
            .as_ref()
            .clone()
            .into()
    };
    fn required_for(schema: &Value) -> &[Value] {
        schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("schema has no required list: {schema}"))
    }

    let create_schema = schema_for("conversation_create");
    assert!(!required_for(&create_schema).contains(&serde_json::json!("approver")));
    assert!(
        create_schema["allOf"].is_array(),
        "Codex create requirements must be conditional"
    );
    let create_and_prompt_schema = schema_for("conversation_create_and_prompt");
    assert!(
        create_and_prompt_schema["allOf"].is_array(),
        "Codex create-and-prompt requirements must be conditional"
    );
    let service_id = "00000000-0000-4000-8000-000000000001";
    let caller = serde_json::json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"schema-caller"
    });
    let provider_create = serde_json::json!({
        "operationId":"019f0000-0000-7000-8000-000000000201",
        "endpoint":{"serviceId":service_id,"endpointId":"cursor-local"},
        "workingDirectory":"/tmp/project","access":"workspace-write","createdBy":caller
    });
    let create_validator = jsonschema::validator_for(&create_schema).expect("create schema");
    assert!(create_validator.is_valid(&provider_create));
    let human = serde_json::json!({"kind":"human","humanId":"fixture-owner"});
    let mut provider_with_human = provider_create.clone();
    provider_with_human["createdBy"] = human.clone();
    provider_with_human["approver"] = human;
    assert!(create_validator.is_valid(&provider_with_human));
    let _: ConversationCreateToolRequest =
        serde_json::from_value(provider_with_human).expect("typed Human MCP create input");
    let mut provider_with_model = provider_create.clone();
    provider_with_model["model"] = serde_json::json!("gpt-5.6-sol");
    assert!(!create_validator.is_valid(&provider_with_model));
    let mut provider_with_effort = provider_create.clone();
    provider_with_effort["effort"] = serde_json::json!("medium");
    assert!(!create_validator.is_valid(&provider_with_effort));
    for (field, value) in [
        ("fork", serde_json::json!("source-thread")),
        (
            "rootMessageId",
            serde_json::json!("019f0000-0000-7000-8000-000000000301"),
        ),
    ] {
        let mut provider_with_codex_only_field = provider_create.clone();
        provider_with_codex_only_field[field] = value;
        assert!(
            !create_validator.is_valid(&provider_with_codex_only_field),
            "provider create schema must reject {field}"
        );
    }
    let mut empty_codex_create = provider_create.clone();
    empty_codex_create["endpoint"]["endpointId"] = serde_json::json!("codex-local");
    assert!(!create_validator.is_valid(&empty_codex_create));
    empty_codex_create["model"] = serde_json::json!("gpt-5.6-sol");
    empty_codex_create["effort"] = serde_json::json!("medium");
    assert!(create_validator.is_valid(&empty_codex_create));

    let create_and_prompt_validator =
        jsonschema::validator_for(&create_and_prompt_schema).expect("create-and-prompt schema");
    let provider_create_and_prompt = serde_json::json!({
        "create":provider_create,
        "message":{"kind":"humanUser","text":"hello"}
    });
    assert!(create_and_prompt_validator.is_valid(&provider_create_and_prompt));
    let mut provider_create_and_prompt_with_effort = provider_create_and_prompt;
    provider_create_and_prompt_with_effort["create"]["effort"] = serde_json::json!("medium");
    assert!(!create_and_prompt_validator.is_valid(&provider_create_and_prompt_with_effort));
    for (field, value) in [
        ("fork", serde_json::json!("source-thread")),
        (
            "rootMessageId",
            serde_json::json!("019f0000-0000-7000-8000-000000000302"),
        ),
    ] {
        let mut provider_create_and_prompt = serde_json::json!({
            "create":provider_create,
            "message":{"kind":"humanUser","text":"hello"}
        });
        provider_create_and_prompt["create"][field] = value;
        assert!(
            !create_and_prompt_validator.is_valid(&provider_create_and_prompt),
            "provider create-and-prompt schema must reject {field}"
        );
    }

    let schedule_schema = schema_for("schedule_create");
    let schedule_definition = &schedule_schema["$defs"]["ScheduleDefinition"];
    assert!(
        schedule_definition["required"]
            .as_array()
            .is_some_and(|fields| fields.contains(&serde_json::json!("effort"))),
        "{schedule_schema}"
    );
    assert!(schedule_definition["allOf"].is_array());
    let mut fresh_schedule = serde_json::json!({
        "operationId":"019f0000-0000-7000-8000-000000000202",
        "definition":{
            "instructionId":"019f0000-0000-7000-8000-000000000203",
            "timing":{"kind":"after","seconds":60},"enabled":false,
            "destination":{"kind":"freshEachRun","endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"cwd":"/tmp/project"},
            "effort":"medium"
        }
    });
    let schedule_validator = jsonschema::validator_for(&schedule_schema).expect("schedule schema");
    assert!(!schedule_validator.is_valid(&fresh_schedule));
    fresh_schedule["definition"]["model"] = serde_json::json!("gpt-5.6-sol");
    assert!(schedule_validator.is_valid(&fresh_schedule));

    let run_schema = schema_for("run_list");
    assert!(!required_for(&run_schema).contains(&serde_json::json!("cursor")));

    let listen = schema_for("board_thread_listen");
    assert!(listen["required"].is_array());
    let descriptions = tools
        .iter()
        .map(|tool| {
            (
                tool.name.as_ref(),
                tool.description.as_deref().unwrap_or_default(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let listen_description = descriptions
        .get("board_thread_listen")
        .expect("listen description");
    assert!(listen_description.contains("--lifetime short"));
    assert!(listen_description.contains("board_thread_join"));
    let join_description = descriptions
        .get("board_thread_join")
        .expect("join description");
    assert!(join_description.contains("--role participant"));

    for tool in ["conversation_create", "conversation_create_and_prompt"] {
        let description = descriptions
            .get(tool)
            .expect("conversation create description");
        assert!(
            description.contains("required for Codex endpoints"),
            "{description}"
        );
        assert!(
            description.contains("rejected for provider endpoints"),
            "{description}"
        );
        assert!(description.contains("rootMessageId"), "{description}");
        assert!(description.contains("fork"), "{description}");
    }
}

#[test]
fn sdk_only_operations_are_advertised_by_both_cli_and_mcp_catalogs() {
    let help = agent_collaboration::command_help();
    for (tool, cli_command) in [
        ("conversation_create", "conversation create"),
        ("conversation_load", "conversation load"),
        ("conversation_create_and_prompt", "conversation prompt"),
        ("conversation_prompt", "conversation prompt"),
        ("conversation_cancel", "conversation cancel"),
        ("events_observe", "events observe"),
    ] {
        assert!(help.contains(cli_command), "CLI adapter missing for {tool}");
    }
}

#[test]
fn conversation_create_tool_preserves_created_and_pending_operation_identity() {
    let operation_id = OperationId::generate();
    let target = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"created-thread"
    }))
    .expect("target");
    let created = conversation_create_tool_result(
        Ok(ConversationCreateOutcome::Created {
            operation_id: operation_id.clone(),
            target,
            effective_settings: None,
        }),
        operation_id.clone(),
    );
    assert_eq!(created.is_error, Some(false));
    assert_eq!(
        created.structured_content.as_ref().unwrap()["kind"],
        "created"
    );
    assert_eq!(
        created.structured_content.as_ref().unwrap()["operationId"],
        serde_json::json!(operation_id)
    );
    assert_eq!(
        created.structured_content.as_ref().unwrap()["target"]["sessionId"],
        "created-thread"
    );

    let pending = conversation_create_tool_result(
        Ok(ConversationCreateOutcome::Pending {
            operation_id: operation_id.clone(),
        }),
        operation_id.clone(),
    );
    assert_eq!(pending.is_error, Some(false));
    assert_eq!(
        pending.structured_content.as_ref().unwrap()["kind"],
        "pending"
    );
    assert_eq!(
        pending.structured_content.as_ref().unwrap()["operationId"],
        serde_json::json!(operation_id)
    );
    assert!(
        pending
            .structured_content
            .as_ref()
            .unwrap()
            .get("target")
            .is_none()
    );
}

#[test]
fn conversation_create_tool_reports_endpoint_named_unsupported_input() {
    let operation_id = OperationId::generate();
    let endpoint = serde_json::from_value(serde_json::json!({
        "serviceId":"00000000-0000-4000-8000-000000000001", "endpointId":"cursor-local"
    }))
    .expect("endpoint");
    let response = conversation_create_tool_result(
        Err(
            collaboration_client::ConversationClientError::UnsupportedInput {
                endpoint,
                field: "model",
                fix: "omit model for cursor-local".to_owned(),
            },
        ),
        operation_id.clone(),
    );
    assert_eq!(response.is_error, Some(true));
    let content = response.structured_content.expect("structured error");
    assert_eq!(content["kind"], "unsupportedCapability");
    assert_eq!(content["operationId"], serde_json::json!(operation_id));
    assert_eq!(content["endpoint"]["endpointId"], "cursor-local");
    assert_eq!(content["field"], "model");
    assert_eq!(content["fix"], "omit model for cursor-local");
}

#[test]
fn representative_catalog_descriptions_explain_operation_specific_behavior() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let descriptions = server
        .resolved_tools()
        .into_iter()
        .map(|tool| {
            (
                tool.name.into_owned(),
                tool.description.unwrap_or_default().into_owned(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    for description in descriptions.values() {
        assert!(!description.contains("Calls the existing typed"));
        assert!(!description.contains("Undocumented collaboration operation"));
    }
    for (name, required_phrases) in [
        ("endpoints_list", &["Read-only"][..]),
        ("message_send", &["accepted", "replayed"][..]),
        (
            "conversation_create_and_prompt",
            &["advertised client", "completed turn", "peer reply"][..],
        ),
        (
            "wake_send",
            &[
                "required operation identity",
                "local scheduling",
                "native input acceptance",
                "agent reply",
            ][..],
        ),
        (
            "schedule_prepare",
            &[
                "Preparation",
                "reuseThread",
                "allocate a native conversation",
                "ReadThread",
                "without loading or resuming",
                "native input acceptance",
            ][..],
        ),
        ("instruction_create", &["required operation identity"][..]),
        ("wake_pause", &["required operation identity"][..]),
        (
            "board_inbox_fetch",
            &[
                "Unread mode initializes reader tracking",
                "Latest mode does not initialize",
            ][..],
        ),
        (
            "board_message_post",
            &["Saving the message", "reply", "assignment success"][..],
        ),
        (
            "board_thread_listen",
            &["Listener readiness", "model activation"][..],
        ),
        ("operation_reconcile", &["never replays"][..]),
    ] {
        let description = descriptions
            .get(name)
            .unwrap_or_else(|| panic!("missing tool {name}"));
        for phrase in required_phrases {
            assert!(
                description.contains(phrase),
                "{name} description missing {phrase:?}: {description}"
            );
        }
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

#[test]
fn advertised_tool_output_schemas_have_object_roots() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let invalid = server
        .resolved_tools()
        .into_iter()
        .filter_map(|tool| {
            let schema = tool.output_schema.as_ref()?;
            let root_type = schema.get("type").and_then(serde_json::Value::as_str);
            let contains_boolean_subschema =
                schema_contains_boolean_subschema(&serde_json::Value::Object((**schema).clone()));
            (root_type != Some("object") || contains_boolean_subschema).then_some((
                tool.name,
                root_type.map(str::to_owned),
                contains_boolean_subschema,
            ))
        })
        .collect::<Vec<_>>();
    assert!(
        invalid.is_empty(),
        "MCP clients require object-root output schemas: {invalid:?}"
    );
}

#[test]
fn described_success_types_keep_their_output_root_description() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    for name in [
        "board_message_show",
        "board_thread_show",
        "board_thread_listen_show",
        "board_thread_listen_cancel",
    ] {
        let schema = server
            .resolved_tools()
            .into_iter()
            .find(|tool| tool.name == name)
            .and_then(|tool| tool.output_schema)
            .expect("described output schema");
        let schema = Value::Object((*schema).clone());
        let success_ref = schema["anyOf"][0]["$ref"]
            .as_str()
            .expect("success branch reference");
        let definition = schema
            .pointer(success_ref.trim_start_matches('#'))
            .expect("success definition");
        assert_eq!(
            schema.get("description"),
            definition.get("description"),
            "{name} root description moved into $defs"
        );
    }
}

fn schema_contains_boolean_subschema(schema: &serde_json::Value) -> bool {
    match schema {
        serde_json::Value::Bool(_) => true,
        serde_json::Value::Object(fields) => {
            [
                "additionalItems",
                "additionalProperties",
                "contains",
                "contentSchema",
                "else",
                "if",
                "items",
                "not",
                "propertyNames",
                "then",
                "unevaluatedItems",
                "unevaluatedProperties",
            ]
            .into_iter()
            .filter_map(|keyword| fields.get(keyword))
            .any(schema_or_array_contains_boolean)
                || ["allOf", "anyOf", "oneOf", "prefixItems"]
                    .into_iter()
                    .filter_map(|keyword| fields.get(keyword).and_then(serde_json::Value::as_array))
                    .flatten()
                    .any(schema_contains_boolean_subschema)
                || [
                    "$defs",
                    "definitions",
                    "dependentSchemas",
                    "patternProperties",
                    "properties",
                ]
                .into_iter()
                .filter_map(|keyword| fields.get(keyword).and_then(serde_json::Value::as_object))
                .flat_map(|subschemas| subschemas.values())
                .any(schema_contains_boolean_subschema)
        }
        _ => false,
    }
}

fn schema_or_array_contains_boolean(schema: &serde_json::Value) -> bool {
    match schema {
        serde_json::Value::Array(items) => items.iter().any(schema_contains_boolean_subschema),
        _ => schema_contains_boolean_subschema(schema),
    }
}

#[test]
fn boolean_schema_normalization_preserves_instance_and_annotation_booleans() {
    let original = serde_json::json!({
        "type":"object",
        "properties":{
            "anything":true,
            "never":false,
            "flag":{
                "type":"boolean",
                "const":true,
                "enum":[true,false],
                "default":true,
                "examples":[false],
                "readOnly":true,
                "deprecated":false
            }
        },
        "required":["anything","flag"],
        "additionalProperties":false
    });
    let mut normalized = original.clone();
    super::normalize_boolean_json_schemas(&mut normalized);
    assert_eq!(normalized["properties"]["anything"], serde_json::json!({}));
    assert_eq!(
        normalized["properties"]["never"],
        serde_json::json!({"not":{}})
    );
    assert_eq!(
        normalized["additionalProperties"],
        serde_json::json!({"not":{}})
    );
    assert_eq!(
        normalized["properties"]["flag"],
        original["properties"]["flag"]
    );

    let original_validator = jsonschema::validator_for(&original).expect("original validator");
    let normalized_validator =
        jsonschema::validator_for(&normalized).expect("normalized validator");
    for instance in [
        serde_json::json!({"anything":"value","flag":true}),
        serde_json::json!({"anything":false,"flag":true}),
        serde_json::json!({"anything":null,"flag":false}),
        serde_json::json!({"anything":1,"flag":true,"extra":true}),
        serde_json::json!({"anything":1,"never":null,"flag":true}),
    ] {
        assert_eq!(
            original_validator.is_valid(&instance),
            normalized_validator.is_valid(&instance),
            "validator meaning changed for {instance}"
        );
    }
    assert!(normalized_validator.is_valid(&serde_json::json!({"anything":0,"flag":true})));
    assert!(!normalized_validator.is_valid(&serde_json::json!({"anything":0,"flag":false})));
    assert!(
        !normalized_validator
            .is_valid(&serde_json::json!({"anything":0,"flag":true,"extra":false}))
    );
}

#[test]
fn advertised_tool_schemas_validate_available_success_and_every_error_sample() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let advertised = server
        .resolved_tools()
        .into_iter()
        .filter_map(|tool| tool.output_schema.map(|schema| (tool.name, schema)))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(advertised.len(), 103);

    let message = serde_json::json!({
        "messageId":"019f0000-0000-7000-8000-000000000001",
        "boardId":"019f0000-0000-7000-8000-000000000002",
        "topicId":"019f0000-0000-7000-8000-000000000003",
        "placement":{"kind":"topic","topicId":"019f0000-0000-7000-8000-000000000003"},
        "actor":{"kind":"human","humanId":"schema-test"},
        "actingFor":null,
        "text":"schema test",
        "references":[],
        "activitySequence":1
    });
    let listen = serde_json::json!({
        "listenId":"019f0000-0000-7000-8000-000000000004",
        "context":{
            "reader":{"kind":"human","humanId":"schema-test"},
            "threads":[],
            "topicIds":[],
            "armedAfterSequence":0
        },
        "mode":{"kind":"once","maxWaitSeconds":1},
        "acknowledge":false,
        "active":false,
        "delivery":"stdout",
        "batchesDelivered":0,
        "firstSequence":null,
        "lastSequence":null,
        "catchUp":false,
        "acknowledged":false,
        "consecutiveRejections":0,
        "lastRejection":null
    });
    let provider_endpoint = serde_json::json!({
        "serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"
    });
    let provider_target =
        serde_json::json!({"endpoint":provider_endpoint,"sessionId":"provider-session"});
    let provider_settings = serde_json::json!({
        "target":provider_target,
        "effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},
            "mappingStatus":"verified","authentication":"authenticated","mode":"ask"}
    });
    let provider_operation = serde_json::json!({
        "operationId":OperationId::generate(), "operation":"conversationResume",
        "binding":{"kind":"externalProvider","binding":{
            "endpoint":provider_endpoint,"bindingId":"binding-1",
            "runtime":{"provider":"claudeCode","runtimeName":"claude-agent-acp"},
            "transport":"stdioAcp",
            "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1},
            "capabilities":[{"name":"prompt","status":"supported","evidence":"advertised"}]
        }},
        "target":provider_target,"stage":"mayHaveDispatched","effect":"unknown",
        "reconciliation":"unresolved","admittedAt":"2026-09-27T00:00:00Z"
    });
    let mut close_operation = provider_operation.clone();
    close_operation["operation"] = serde_json::json!("conversationClose");
    let valid_instances = std::collections::BTreeMap::from([
        ("approval_list", serde_json::json!({"approvals":[]})),
        ("board_message_show", message),
        ("board_thread_listen_cancel", listen.clone()),
        ("board_thread_listen_show", listen),
        (
            "conversation_create",
            serde_json::json!({"kind":"pending","operationId":OperationId::generate()}),
        ),
        (
            "conversation_create_and_prompt",
            serde_json::json!({"kind":"createPending","operationId":OperationId::generate()}),
        ),
        (
            "conversation_load",
            serde_json::json!({"kind":"pending","operationId":OperationId::generate(),
                "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"fixture"}}),
        ),
        (
            "conversation_close",
            serde_json::json!({"admission":"admitted","operation":close_operation}),
        ),
        (
            "conversation_resume",
            serde_json::json!({"admission":"admitted","operation":provider_operation}),
        ),
        ("conversation_settings_set", provider_settings.clone()),
        ("conversation_settings_accept", provider_settings),
        (
            "conversation_prompt",
            serde_json::json!({"kind":"pending","operationId":OperationId::generate(),
                "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"fixture"}}),
        ),
        (
            "journal_status",
            serde_json::json!({"storage":"unavailable"}),
        ),
        (
            "provider_sessions_list",
            serde_json::json!({
                "endpoint":provider_endpoint,"observedAt":"2026-09-27T00:00:00Z",
                "sessions":[
                    {
                        "origin":"hostedProvider","target":provider_target,
                        "workingDirectory":"/tmp/project","updatedAt":1790162500,
                        "state":"idle","approver":{"kind":"human","humanId":"owner"},
                        "createdBy":{"kind":"human","humanId":"owner"}
                    },
                    {
                        "origin":"claudeCodeInteractive",
                        "target":{"endpoint":provider_endpoint,"sessionId":"interactive-session"},
                        "name":"Claude terminal","workingDirectory":"/tmp/project",
                        "status":"busy","startedAt":1790162400,"updatedAt":1790162494,
                        "statusUpdatedAt":1790162494,"kind":"claudeCode","entrypoint":"cli"
                    }
                ],"skippedRecords":0
            }),
        ),
        (
            "provider_session_inspect",
            serde_json::json!({
                "target":provider_target,"state":"idle","history":"available",
                "capabilities":{"load":true,"resume":true,"close":true,"list":true,"steer":false,
                    "queue":null,"modes":false,"configOptions":false,"elicitation":false,
                    "usage":false,"promptContent":{"image":false,"audio":false,"embeddedContext":false},
                    "authStatus":{"kind":"apiKey","label":"API key"}}
            }),
        ),
        ("question_list", serde_json::json!({"questions":[]})),
        (
            "question_answer",
            serde_json::json!({"requestId":"question-1","state":"cancelled"}),
        ),
        (
            "events_observe",
            serde_json::json!({
                "target":provider_target,
                "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1},
                "attached":true,"events":[],"endReason":"deadlineReached","continuationGap":false,"epoch":1
            }),
        ),
    ]);
    let non_objects = [
        serde_json::Value::Null,
        serde_json::json!(false),
        serde_json::json!(0),
        serde_json::json!("value"),
        serde_json::json!([]),
    ];
    for (name, advertised_schema) in advertised {
        assert_eq!(
            advertised_schema.get("type"),
            Some(&serde_json::json!("object"))
        );
        let advertised_value = serde_json::Value::Object((*advertised_schema).clone());
        let advertised_validator =
            jsonschema::validator_for(&advertised_value).expect("advertised catalog validator");
        assert_eq!(
            advertised_value.get("$schema"),
            Some(&serde_json::json!(
                "https://json-schema.org/draft/2020-12/schema"
            )),
            "{name} schema draft"
        );
        if let Some(valid) = valid_instances.get(name.as_ref()) {
            advertised_validator
                .validate(valid)
                .unwrap_or_else(|error| {
                    panic!("success result narrowed for {name}: {error}; {valid}")
                });
        }
        if name.as_ref() == "approval_list" {
            let requester = serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
                "sessionId":"requester"
            });
            for result in [
                serde_json::json!({"approvals":[{
                    "requestId":"legacy", "requester":requester, "approver":requester,
                    "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1},
                    "state":"pendingClientDecision", "decision":null, "operation":{},
                    "expiresAt":"2026-09-27T00:00:00Z"
                }]}),
                serde_json::json!({"approvals":[{
                    "requestId":"refused", "requester":requester,
                    "approver":{"kind":"human","humanId":"owner"},
                    "state":"refused", "reason":"malformed options", "title":"Run command",
                    "description":null, "options":[]
                }]}),
            ] {
                assert!(
                    advertised_validator.is_valid(&result),
                    "approval list success branch rejected {result}"
                );
            }
        }
        let error = super::structured_tool_error(serde_json::json!({
            "kind":"protocolViolation", "stage":"validation", "effect":"none",
            "message":"fixture validation failure", "code":null, "data":null
        }));
        let structured = error.structured_content.expect("structured error sample");
        advertised_validator
            .validate(&structured)
            .unwrap_or_else(|failure| {
                panic!("{name} error result violates advertised schema: {failure}; {structured}")
            });
        for instance in &non_objects {
            assert!(
                !advertised_validator.is_valid(instance),
                "{name} unexpectedly admits non-object {instance}"
            );
        }
    }
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

fn expected_tool_name(method: &str) -> String {
    if let Some(provider_method) = method.strip_prefix("conversation/") {
        return format!(
            "conversation_{}",
            provider_method
                .chars()
                .flat_map(|character| {
                    if character.is_ascii_uppercase() {
                        vec!['_', character.to_ascii_lowercase()]
                    } else {
                        vec![character]
                    }
                })
                .collect::<String>()
        );
    }
    match method {
        "endpoint/list" => return "endpoints_list".to_owned(),
        "codex/sessionList" => return "sessions_list".to_owned(),
        "provider/sessionList" => return "provider_sessions_list".to_owned(),
        "provider/sessionInspect" => return "provider_session_inspect".to_owned(),
        "codex/sessionInspect" => return "session_inspect".to_owned(),
        "codex/sessionRename" => return "session_rename".to_owned(),
        "message/send" => return "message_send".to_owned(),
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
#[test]
fn success_schema_branches_match_main_golden_snapshot() {
    // Generated by resolved_tools() at origin/main d8db284c, before the union cutover.
    let expected: std::collections::BTreeMap<String, Value> =
        serde_json::from_str(include_str!("snapshots/main_success_schemas.json"))
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
    assert_eq!(actual.len(), 103);
    assert_eq!(expected.len(), 103);
    for (name, success) in actual {
        assert_eq!(success, expected[&name], "{name} success schema drifted");
    }
}
