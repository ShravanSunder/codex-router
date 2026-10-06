use super::*;

#[test]
fn router_push_tools_expose_reported_caller_and_reference_contracts() {
    let server = CollaborationMcpServer::new(std::env::temp_dir());
    let tools = server.resolved_tools();

    for (name, required_fields) in [
        ("router_show", &["caller", "reference"][..]),
        ("message_inbox", &["caller", "limit"][..]),
        ("message_history", &["caller", "with", "limit"][..]),
        ("message_reply", &["caller", "reference", "text"][..]),
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("missing {name} tool"));
        let input_schema = Value::Object((*tool.input_schema).clone());
        let required = input_schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} required input fields"));
        for field in required_fields {
            assert!(
                required.contains(&serde_json::json!(field)),
                "{name} must require {field}"
            );
        }
        assert!(
            input_schema["properties"].get("expectSender").is_none(),
            "{name} must not retain expectSender"
        );
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(description.contains("reported"), "{name}: {description}");
        assert!(
            description.contains("not authenticated"),
            "{name}: {description}"
        );
    }

    let message_send = tools
        .iter()
        .find(|tool| tool.name == "message_send")
        .expect("message_send tool");
    let description = message_send.description.as_deref().unwrap_or_default();
    assert!(description.contains("reported"), "{description}");
    assert!(description.contains("not authenticated"), "{description}");
    let output_schema = Value::Object(
        (**message_send
            .output_schema
            .as_ref()
            .expect("message_send output schema"))
        .clone(),
    );
    let success_definition = output_schema["anyOf"][0]["$ref"]
        .as_str()
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
        .expect("message_send success definition");
    let required = output_schema["$defs"][success_definition]["required"]
        .as_array()
        .expect("message_send success fields");
    for field in ["pushId", "link", "target", "deliveryState", "receipt"] {
        assert!(
            required.contains(&serde_json::json!(field)),
            "message_send result must require {field}"
        );
    }
}

#[tokio::test]
async fn message_send_returns_push_identity_and_forwards_the_reported_agent_sender() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let target = fixture_session("claude-local", "target-session");
    let sender = fixture_session("codex-local", "reported-sender");
    let expected_result = serde_json::json!({
        "pushId":FIXTURE_PUSH_ID,
        "link":format!("router://{FIXTURE_SERVICE_ID}/push/{FIXTURE_PUSH_ID}"),
        "target":target,
        "targetIdentity":"✳️ claude-local/target-s",
        "deliveryState":"delivered",
        "receipt":{
            "outcome":{"kind":"peerMessageWritten"},
            "reachability":"claudeCodePeer",
            "client":{"kind":"claudeCodePeer"}
        }
    });
    let peer = start_control_response_fixture(
        temporary.path(),
        "message/send",
        Ok(expected_result.clone()),
    );
    let request: collaboration_client::MessageSendRequest =
        serde_json::from_value(serde_json::json!({
            "target":target,
            "message":{"kind":"agent","sender":sender,"text":"reporting identity"},
            "delivery":"auto",
            "generationGuard":null
        }))
        .expect("message_send input");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());

    let result = server.message_send(super::super::Parameters(request)).await;
    let control_request = finish_control_fixture(peer).await;

    assert_ne!(result.is_error, Some(true));
    assert_eq!(control_request["params"]["message"]["sender"], sender);
    let structured = result.structured_content.expect("push result");
    assert_eq!(structured, expected_result);
    assert_eq!(structured["pushId"], FIXTURE_PUSH_ID);
    assert_eq!(structured["link"], expected_result["link"]);
    assert_eq!(structured["target"], target);
    assert_eq!(
        structured["receipt"]["outcome"]["kind"],
        "peerMessageWritten"
    );
}

#[tokio::test]
async fn router_show_forwards_caller_and_preserves_control_read_state() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let caller = fixture_session("codex-local", "target-session");
    let expected_result = direct_message_show_result(caller.clone());
    let peer = start_control_response_fixture(
        temporary.path(),
        "router/show",
        Ok(expected_result.clone()),
    );
    let request: collaboration_protocol::PushRecordShowParams =
        serde_json::from_value(serde_json::json!({"caller":caller,"reference":FIXTURE_PUSH_ID}))
            .expect("router_show input");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());

    let result = server.router_show(super::super::Parameters(request)).await;
    let control_request = finish_control_fixture(peer).await;

    assert_ne!(result.is_error, Some(true));
    assert_eq!(control_request["params"]["caller"], caller);
    assert_eq!(control_request["params"]["reference"], FIXTURE_PUSH_ID);
    let structured = result.structured_content.expect("shown record");
    assert_eq!(structured, expected_result);
    assert_eq!(structured["record"]["readAt"], "2026-09-30T16:00:02Z");
}

#[tokio::test]
async fn router_show_preserves_not_permitted_control_error() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let caller = fixture_session("cursor-local", "unrelated-session");
    let peer = start_control_response_fixture(
        temporary.path(),
        "router/show",
        Err(serde_json::json!({
            "code":-32050,
            "message":"not permitted",
            "data":{
                "kind":"notPermitted",
                "stage":"inspect",
                "message":"only a direct-message participant may read this record"
            }
        })),
    );
    let request: collaboration_protocol::PushRecordShowParams =
        serde_json::from_value(serde_json::json!({"caller":caller,"reference":FIXTURE_PUSH_ID}))
            .expect("router_show input");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());

    let result = server.router_show(super::super::Parameters(request)).await;
    let control_request = finish_control_fixture(peer).await;

    assert_eq!(result.is_error, Some(true));
    assert_eq!(control_request["params"]["caller"], caller);
    let structured = result.structured_content.expect("typed read error");
    assert_eq!(structured["serviceKind"], "notPermitted");
    assert_eq!(structured["stage"], "inspect");
    assert_eq!(
        structured["message"],
        "only a direct-message participant may read this record"
    );
}

#[tokio::test]
async fn message_inbox_forwards_reported_caller_and_returns_unread_records() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let caller = fixture_session("codex-local", "target-session");
    let expected_result = serde_json::json!({"records":[]});
    let peer = start_control_response_fixture(
        temporary.path(),
        "message/inbox",
        Ok(expected_result.clone()),
    );
    let request: collaboration_protocol::PushRecordListParams =
        serde_json::from_value(serde_json::json!({"caller":caller,"limit":20}))
            .expect("message_inbox input");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());

    let result = server
        .message_inbox(super::super::Parameters(request))
        .await;
    let control_request = finish_control_fixture(peer).await;

    assert_ne!(result.is_error, Some(true));
    assert_eq!(control_request["params"]["caller"], caller);
    assert_eq!(control_request["params"]["limit"], 20);
    assert_eq!(result.structured_content, Some(expected_result));
}

#[tokio::test]
async fn message_history_forwards_both_reported_sessions() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let caller = fixture_session("codex-local", "target-session");
    let with = fixture_session("claude-local", "sender-session");
    let expected_result = serde_json::json!({"records":[]});
    let peer = start_control_response_fixture(
        temporary.path(),
        "message/history",
        Ok(expected_result.clone()),
    );
    let request: collaboration_protocol::PushRecordHistoryParams =
        serde_json::from_value(serde_json::json!({"caller":caller,"with":with,"limit":20}))
            .expect("message_history input");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());

    let result = server
        .message_history(super::super::Parameters(request))
        .await;
    let control_request = finish_control_fixture(peer).await;

    assert_ne!(result.is_error, Some(true));
    assert_eq!(control_request["params"]["caller"], caller);
    assert_eq!(control_request["params"]["with"], with);
    assert_eq!(control_request["params"]["limit"], 20);
    assert_eq!(result.structured_content, Some(expected_result));
}

#[tokio::test]
async fn message_reply_targets_the_referenced_push_without_expect_sender() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let caller = fixture_session("codex-local", "target-session");
    let recipient = fixture_session("claude-local", "sender-session");
    let reply_push_id = "018f1f12-3456-7abc-8def-0123456789ac";
    let expected_result = serde_json::json!({
        "target":recipient,
        "targetIdentity":"✳️ claude-local/sender-s",
        "pushId":reply_push_id,
        "link":format!("router://{FIXTURE_SERVICE_ID}/push/{reply_push_id}"),
        "deliveryState":"delivered",
        "receipt":{
            "outcome":{"kind":"peerMessageWritten"},
            "reachability":"claudeCodePeer",
            "client":{"kind":"claudeCodePeer"}
        }
    });
    let peer = start_control_response_fixture(
        temporary.path(),
        "message/reply",
        Ok(expected_result.clone()),
    );
    let reference = format!("router://{FIXTURE_SERVICE_ID}/push/{FIXTURE_PUSH_ID}");
    let request: collaboration_client::MessageReplyRequest = serde_json::from_value(
        serde_json::json!({"caller":caller,"reference":reference,"text":"reply by id"}),
    )
    .expect("message_reply input");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());

    let result = server
        .message_reply(super::super::Parameters(request))
        .await;
    let control_request = finish_control_fixture(peer).await;

    assert_ne!(result.is_error, Some(true));
    assert_eq!(control_request["params"]["caller"], caller);
    assert_eq!(control_request["params"]["reference"], reference);
    assert_eq!(control_request["params"]["text"], "reply by id");
    assert!(control_request["params"].get("expectSender").is_none());
    let structured = result.structured_content.expect("reply result");
    assert_eq!(structured, expected_result);
}

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
    let result = server
        .sessions_list(super::super::Parameters(request))
        .await;
    assert_eq!(result.is_error, Some(true));
    let content = serde_json::to_string(&result.structured_content).expect("error content");
    assert!(content.contains("provider_sessions_list"), "{content}");
}

#[test]
fn message_send_route_push_results_match_advertised_output_schema() {
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
    let target = fixture_session("codex-local", "thread-a");
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
    for (route, receipt) in cases {
        let value = serde_json::json!({
            "pushId":FIXTURE_PUSH_ID,
            "link":format!("router://{FIXTURE_SERVICE_ID}/push/{FIXTURE_PUSH_ID}"),
            "target":target,
            "targetIdentity":"✳️ codex-local/threa",
            "deliveryState":"delivered",
            "receipt":receipt
        });
        let push_result: collaboration_protocol::PushMessageSendResult =
            serde_json::from_value(value.clone())
                .unwrap_or_else(|error| panic!("{route} push result fixture: {error}"));
        let result = super::super::message_tool_result(Ok(push_result));
        assert_ne!(result.is_error, Some(true), "{route} tool result");
        let structured = result.structured_content.expect("structured receipt");
        assert_eq!(structured, value, "{route} success wire shape changed");
        validator.validate(&structured).unwrap_or_else(|error| {
            panic!("{route} structuredContent violates advertised schema: {error}; {structured}")
        });
    }
}

#[test]
fn message_send_held_delivery_is_structured_success() {
    let link = format!("router://{FIXTURE_SERVICE_ID}/push/{FIXTURE_PUSH_ID}");
    let push_result: collaboration_protocol::PushMessageSendResult =
        serde_json::from_value(serde_json::json!({
            "pushId":FIXTURE_PUSH_ID,
            "link":link,
            "target":fixture_session("codex-local", "thread-a"),
            "targetIdentity":"✳️ codex-local/thread-a",
            "deliveryState":"held",
            "receipt":{
                "outcome":{"kind":"notSubmitted","retryable":true,"reason":"target is not running"},
                "reachability":null,
                "client":null
            }
        }))
        .expect("typed held push result");

    let result = super::super::message_tool_result(Ok(push_result));

    assert_ne!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured held success");
    assert_eq!(structured["deliveryState"], "held");
    assert_eq!(structured["link"], link);
    assert_eq!(structured["receipt"]["outcome"]["kind"], "notSubmitted");
}

#[test]
fn message_reply_is_by_reference_and_has_no_latest_sender_compatibility_fields() {
    let server = super::super::CollaborationMcpServer::new(std::env::temp_dir());
    let tool = server
        .resolved_tools()
        .into_iter()
        .find(|tool| tool.name == "message_reply")
        .expect("advertised message_reply tool");
    let input_schema = serde_json::Value::Object((*tool.input_schema).clone());
    let required = input_schema
        .get("required")
        .and_then(serde_json::Value::as_array)
        .expect("reply required fields");
    assert!(required.iter().any(|field| field == "caller"));
    assert!(required.iter().any(|field| field == "reference"));
    assert!(required.iter().any(|field| field == "text"));
    assert!(input_schema["properties"].get("expectSender").is_none());
    let stale_latest_sender_input =
        serde_json::from_value::<collaboration_client::MessageReplyRequest>(serde_json::json!({
            "caller":fixture_session("codex-local", "target-session"),
            "reference":FIXTURE_PUSH_ID,
            "text":"reply",
            "expectSender":fixture_session("claude-local", "sender-session")
        }));
    assert!(
        stale_latest_sender_input.is_err(),
        "message_reply must reject expectSender instead of retaining a compatibility path"
    );

    let output_schema = serde_json::Value::Object(
        (**tool.output_schema.as_ref().expect("reply output schema")).clone(),
    );
    let validator = jsonschema::validator_for(&output_schema).expect("reply output schema");
    let valid_reply_result = serde_json::json!({
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"sender-session"},
        "targetIdentity":"✳️ claude-local/sender-s",
        "pushId":FIXTURE_PUSH_ID,
        "link":format!("router://{FIXTURE_SERVICE_ID}/push/{FIXTURE_PUSH_ID}"),
        "deliveryState":"delivered",
        "receipt":{"outcome":{"kind":"peerMessageWritten"},
            "reachability":"claudeCodePeer",
            "client":{"kind":"claudeCodePeer"}}
    });
    assert!(validator.is_valid(&valid_reply_result));
}

#[test]
fn message_reply_held_delivery_is_structured_success() {
    let link = format!("router://{FIXTURE_SERVICE_ID}/push/{FIXTURE_PUSH_ID}");
    let reply_result: collaboration_protocol::SessionMessageReplyResult =
        serde_json::from_value(serde_json::json!({
            "target":fixture_session("claude-local", "sender-session"),
            "targetIdentity":"✳️ claude-local/sender-session",
            "pushId":FIXTURE_PUSH_ID,
            "link":link,
            "deliveryState":"held",
            "receipt":{
                "outcome":{"kind":"notSubmitted","retryable":true,"reason":"target is not running"},
                "reachability":null,
                "client":null
            }
        }))
        .expect("typed held reply result");

    let result = super::super::message_reply_tool_result(Ok(reply_result));

    assert_ne!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured held success");
    assert_eq!(structured["deliveryState"], "held");
    assert_eq!(structured["link"], link);
    assert_eq!(structured["receipt"]["outcome"]["kind"], "notSubmitted");
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
    let result = super::super::message_tool_result(Err(
        collaboration_client::MessageSendError::Submission {
            target,
            source: Box::new(collaboration_client::ClientError::Protocol(
                "invalid message receipt; acceptance unknown",
            )),
        },
    ));
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
    let push_result: collaboration_protocol::PushMessageSendResult =
        serde_json::from_value(serde_json::json!({
            "pushId":FIXTURE_PUSH_ID,
            "link":format!("router://{FIXTURE_SERVICE_ID}/push/{FIXTURE_PUSH_ID}"),
            "target":fixture_session("codex-local", "thread-a"),
            "targetIdentity":"✳️ codex-local/thread-a",
            "deliveryState":"outcome-unknown",
            "receipt":{"outcome":{"kind":"unknown"},"reachability":null,"client":null}
        }))
        .expect("typed unknown push result");
    let result = super::super::message_tool_result(Ok(push_result));
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error receipt");
    assert_eq!(structured["mcpResult"], "error");
    assert_eq!(structured["kind"], "outcomeUnknown");
    assert_eq!(structured["effect"], "unknown");
    assert_eq!(structured["pushId"], FIXTURE_PUSH_ID);
    assert_eq!(structured["receipt"]["outcome"]["kind"], "unknown");
    validator.validate(&structured).expect("typed error schema");
}

#[test]
fn thread_wait_unknown_outcome_retains_inspection_identity_and_guidance() {
    let actor: message_board::Identity = serde_json::from_value(serde_json::json!({
        "kind":"session",
        "session":fixture_session("codex-local", "wait-reader")
    }))
    .expect("wait Reader identity");
    let filter: collaboration_protocol::ThreadSubscriptionWaitFilter =
        serde_json::from_value(serde_json::json!({
            "kind":"roots",
            "rootMessageIds":["019f0000-0000-7000-8000-000000000005"]
        }))
        .expect("thread-root wait filter");
    let result: Result<
        collaboration_protocol::ThreadSubscriptionWaitResult,
        collaboration_client::BoardClientError,
    > = Err(collaboration_client::BoardClientError::WaitOutcomeUnknown {
        actor: actor.clone(),
        filter: filter.clone(),
    });
    let tool_result = super::super::board_result(result, true);
    assert_eq!(tool_result.is_error, Some(true));
    let structured = tool_result
        .structured_content
        .expect("typed wait outcome error");
    assert_eq!(structured["mcpResult"], "error");
    assert_eq!(structured["kind"], "outcomeUnknown");
    assert_eq!(structured["stage"], "response");
    assert_eq!(structured["effect"], "unknown");
    assert_eq!(
        structured["actor"],
        serde_json::to_value(actor).expect("actor JSON")
    );
    assert_eq!(
        structured["filter"],
        serde_json::to_value(filter).expect("filter JSON")
    );
    assert!(structured.get("resource").is_none());
    let message = structured["message"].as_str().expect("recovery guidance");
    for phrase in [
        "activity may have been handed off",
        "subscriptions",
        "unread inbox",
        "board thread subscriptions",
        "board inbox fetch",
    ] {
        assert!(message.contains(phrase), "{message}");
    }

    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let schema = server
        .resolved_tools()
        .into_iter()
        .find(|tool| tool.name == "board_thread_wait")
        .and_then(|tool| tool.output_schema)
        .expect("advertised board_thread_wait output schema");
    let schema = Value::Object((*schema).clone());
    jsonschema::validator_for(&schema)
        .expect("board_thread_wait output schema")
        .validate(&structured)
        .expect("typed wait outcome matches advertised MCP output schema");
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
    let push_result: collaboration_protocol::PushMessageSendResult =
        serde_json::from_value(serde_json::json!({
            "pushId":FIXTURE_PUSH_ID,
            "link":format!("router://{FIXTURE_SERVICE_ID}/push/{FIXTURE_PUSH_ID}"),
            "target":fixture_session("codex-local", "thread-a"),
            "targetIdentity":"✳️ codex-local/thread-a",
            "deliveryState":"rejected",
            "receipt":{
                "outcome":{
                    "kind":"rejected","reason":"heldByAnotherClient",
                    "nextAction":"messageFromHoldingCodexClient","clientCode":-32600,
                    "detail":"Message it from the Codex client that holds it."
                },
                "reachability":"codexAppServer","client":null
            }
        }))
        .expect("typed rejected push result");
    let result = super::super::message_tool_result(Ok(push_result));
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured MCP error");
    assert_eq!(structured["mcpResult"], "error");
    assert_eq!(structured["pushId"], FIXTURE_PUSH_ID);
    assert_eq!(
        structured["receipt"]["outcome"]["reason"],
        "heldByAnotherClient"
    );
    assert_eq!(
        structured["receipt"]["outcome"]["nextAction"],
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
    let preparation = super::super::message_tool_result(Err(
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
    let submission = super::super::message_tool_result(Err(
        collaboration_client::MessageSendError::Submission {
            target: target.clone(),
            source: Box::new(transport()),
        },
    ));
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

    let rejection = super::super::message_tool_result(Err(
        collaboration_client::MessageSendError::Preparation(Box::new(
            collaboration_client::ClientError::Rejected {
                code: -32050,
                data: Some(serde_json::json!({
                    "kind":"staleGeneration",
                    "stage":"discovery",
                    "expected":2
                })),
            },
        )),
    ));
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
    let unsupported = super::super::structured_result::<serde_json::Value>(
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

    let lost_after_send = super::super::structured_result::<serde_json::Value>(
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
