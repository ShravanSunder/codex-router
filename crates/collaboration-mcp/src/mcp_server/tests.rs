use super::{CollaborationMcpServer, conversation_create_tool_result};
use collaboration_protocol::{ConversationCreateOutcome, OperationId};
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
    assert_eq!(tools.len(), 95);
    let mut names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 95);
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
    let mut empty_codex_create = provider_create;
    empty_codex_create["endpoint"]["endpointId"] = serde_json::json!("codex-local");
    assert!(!create_validator.is_valid(&empty_codex_create));
    empty_codex_create["model"] = serde_json::json!("gpt-5.6-sol");
    empty_codex_create["effort"] = serde_json::json!("medium");
    assert!(create_validator.is_valid(&empty_codex_create));

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
fn advertised_object_roots_preserve_original_catalog_semantics() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let original = server
        .tool_router
        .list_all()
        .into_iter()
        .filter_map(|tool| {
            let schema = tool.output_schema?;
            (schema.get("type").is_none()).then_some((tool.name, schema))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let advertised = server
        .resolved_tools()
        .into_iter()
        .filter_map(|tool| tool.output_schema.map(|schema| (tool.name, schema)))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        original
            .keys()
            .map(AsRef::<str>::as_ref)
            .collect::<Vec<_>>(),
        vec![
            "board_message_show",
            "board_thread_listen_cancel",
            "board_thread_listen_show",
            "conversation_create",
            "conversation_create_and_prompt",
            "conversation_load",
            "conversation_prompt",
            "journal_status"
        ]
    );

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
    let valid_instances = std::collections::BTreeMap::from([
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
            "conversation_prompt",
            serde_json::json!({"kind":"pending","operationId":OperationId::generate(),
                "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"fixture"}}),
        ),
        (
            "journal_status",
            serde_json::json!({"storage":"unavailable"}),
        ),
    ]);
    let non_objects = [
        serde_json::Value::Null,
        serde_json::json!(false),
        serde_json::json!(0),
        serde_json::json!("value"),
        serde_json::json!([]),
    ];
    for (name, original_schema) in original {
        let advertised_schema = advertised.get(&name).expect("advertised schema");
        assert_eq!(
            advertised_schema.get("type"),
            Some(&serde_json::json!("object"))
        );
        let original_value = serde_json::Value::Object((*original_schema).clone());
        let advertised_value = serde_json::Value::Object((**advertised_schema).clone());
        let original_validator =
            jsonschema::validator_for(&original_value).expect("original catalog validator");
        let advertised_validator =
            jsonschema::validator_for(&advertised_value).expect("advertised catalog validator");
        let valid = valid_instances
            .get(name.as_ref())
            .expect("valid tool result");
        assert!(
            original_validator.is_valid(valid),
            "invalid original fixture for {name}"
        );
        assert!(
            advertised_validator.is_valid(valid),
            "valid result narrowed for {name}"
        );
        for instance in &non_objects {
            assert_eq!(
                original_validator.is_valid(instance),
                advertised_validator.is_valid(instance),
                "root meaning changed for {name} and {instance}"
            );
            assert!(
                !original_validator.is_valid(instance),
                "{name} unexpectedly admitted non-object {instance} before normalization"
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
