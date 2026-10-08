use super::*;

#[test]
fn native_schema_refs_bind_when_advertised_and_remain_honestly_opaque_otherwise() {
    let source = serde_json::json!({"properties":{"thread":{"$ref":"urn:codex-native:Thread"}}});
    let mut unavailable = source.clone();
    super::super::bind_native_schema_refs(&mut unavailable, None);
    assert_eq!(
        unavailable.pointer("/properties/thread"),
        Some(&serde_json::json!({}))
    );

    let definitions = serde_json::Map::from_iter([(
        "Thread".to_owned(),
        serde_json::json!({"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}),
    )]);
    let mut available = source;
    super::super::bind_native_schema_refs(&mut available, Some(&definitions));
    assert_eq!(
        available.pointer("/properties/thread/$ref"),
        Some(&serde_json::json!("#/$defs/codexNativeThread"))
    );
    jsonschema::validator_for(&available).expect("bound native schema compiles");
}

#[test]
fn advertised_native_bundle_is_loaded_into_discovered_tool_schema() {
    let document = serde_json::json!({
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
    });
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(
        std::collections::BTreeMap::from([(
            "codex_app_server_protocol.schemas.json".to_owned(),
            serde_json::to_vec(&document).expect("native schema JSON"),
        )]),
    )
    .expect("native schema bundle");
    let definitions =
        crate::NativeSchemaDefinitions::from_bundle(&bundle).expect("bundle v2 definitions");
    assert_eq!(
        definitions.digest(),
        codex_native_integration::NativeSchemaDigest::from(&bundle)
    );
    let surface = super::super::ToolSurface::new(Some(&definitions));
    let inspect = surface
        .tools()
        .iter()
        .find(|tool| tool.name == "session_inspect")
        .cloned()
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

    let board = surface
        .tools()
        .iter()
        .find(|tool| tool.name == "board_list")
        .expect("board list tool");
    let board_schema = serde_json::to_string(&board.input_schema).expect("board schema encoding");
    assert!(!board_schema.contains("codexNative"));
}

#[test]
fn advertised_tool_output_schemas_have_object_roots() {
    let server = CollaborationMcpServer::catalog_only();
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
    let server = CollaborationMcpServer::catalog_only();
    for name in ["board_message_show", "board_thread_show"] {
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
    super::super::normalize_boolean_json_schemas(&mut normalized);
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
    let server = CollaborationMcpServer::catalog_only();
    let advertised = server
        .resolved_tools()
        .into_iter()
        .filter_map(|tool| tool.output_schema.map(|schema| (tool.name, schema)))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(advertised.len(), 107);

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
    let pending_root = serde_json::json!({
        "rootId":"019f0000-0000-7000-8000-000000000006",
        "topicId":"019f0000-0000-7000-8000-000000000007",
        "fromSequence":1,"throughSequence":1,"messageCount":1
    });
    let wait_notice = serde_json::json!({
        "batch":{
            "kind":"notice","pushId":"019f0000-0000-7000-8000-000000000004",
            "line":"🧵 Router: new thread activity @fixture · 1 thread · 1 message · router://fixture/push/019f0000-0000-7000-8000-000000000004",
            "held":false,"draining":false,"roots":[pending_root]
        }
    });
    let wait_ranges = serde_json::json!({
        "batch":{
            "kind":"ranges","held":true,"heldSince":"2026-10-01T00:00:00Z",
            "draining":false,"roots":[pending_root]
        }
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
    let active_subscription = serde_json::json!({
        "scope":{"kind":"thread","rootMessageId":"019f0000-0000-7000-8000-000000000005"},
        "policy":{"mode":"deliver","whenIdle":"hold",
            "timing":{"quietSeconds":120,"capSeconds":600},"lifetime":86400},
        "state":"active","expiresAt":"2026-09-28T00:00:00Z","pendingCount":0,
        "presence":{"kind":"running"}
    });
    let ended_subscription = serde_json::json!({
        "scope":{"kind":"thread","rootMessageId":"019f0000-0000-7000-8000-000000000005"},
        "policy":{"mode":"deliver","whenIdle":"hold",
            "timing":{"quietSeconds":120,"capSeconds":600},"lifetime":86400},
        "state":"ended","endReason":{"kind":"cancelled"},
        "expiresAt":"2026-09-28T00:00:00Z","pendingCount":0,
        "presence":{"kind":"running"}
    });
    let valid_instances = std::collections::BTreeMap::from([
        ("approval_list", serde_json::json!({"approvals":[]})),
        ("board_message_show", message),
        ("board_thread_subscribe", active_subscription.clone()),
        ("board_thread_unsubscribe", ended_subscription),
        (
            "board_thread_subscriptions",
            serde_json::json!({"subscriptions":[active_subscription]}),
        ),
        ("board_thread_wait", serde_json::json!({"batch":null})),
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
        if name.as_ref() == "board_thread_wait" {
            for valid in [&wait_notice, &wait_ranges] {
                advertised_validator
                    .validate(valid)
                    .unwrap_or_else(|error| panic!("wait batch result narrowed: {error}; {valid}"));
            }
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
        let error = super::super::structured_tool_error(serde_json::json!({
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
