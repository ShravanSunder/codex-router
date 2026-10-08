use super::*;

#[test]
fn router_push_tools_expose_reported_caller_and_reference_contracts() {
    let server = CollaborationMcpServer::catalog_only();
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

/// Session delivery that writes every message to a Claude Code peer.
struct PeerWrittenDelivery;

impl collaboration_service::SessionMessageDelivery for PeerWrittenDelivery {
    fn deliver<'a>(
        &'a self,
        _request: collaboration_service::layer_zero::DeliveryRequest,
        _evidence: &'a dyn collaboration_service::AttemptEvidenceSink,
    ) -> collaboration_service::DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt> {
        Box::pin(async {
            Ok(collaboration_protocol::DeliveryReceipt {
                outcome: collaboration_protocol::DeliveryOutcome::PeerMessageWritten,
                reachability: Some(collaboration_protocol::SessionReachability::ClaudeCodePeer),
                client: Some(collaboration_protocol::DeliveryClientReceipt::ClaudeCodePeer),
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _context: collaboration_service::AttemptReconciliationContext,
    ) -> collaboration_service::DeliveryFuture<'_, collaboration_service::AttemptReconciliation>
    {
        Box::pin(async { Ok(collaboration_service::AttemptReconciliation::StillUnknown) })
    }
}

/// A server over a Router with a real push store, its delivery owner and peer delivery.
async fn message_server(
    directory: &std::path::Path,
) -> (
    CollaborationMcpServer,
    collaboration_service::SubscriptionDeliveryService,
) {
    let store = std::sync::Arc::new(tokio::sync::Mutex::new(
        automation_storage::AutomationStore::open(&directory.join("automation.sqlite"))
            .await
            .expect("push store"),
    ));
    let delivery: std::sync::Arc<dyn collaboration_service::SessionMessageDelivery> =
        std::sync::Arc::new(PeerWrittenDelivery);
    let presence: std::sync::Arc<dyn collaboration_service::TargetPresenceProbe> =
        std::sync::Arc::new(collaboration_service::SessionDeliveryRouter::new(Vec::new()));
    let owner = collaboration_service::SubscriptionDeliveryService::new(
        collaboration_service::SubscriptionDeliveryServiceProps {
            board_availability: collaboration_service::BoardAvailability::Unavailable,
            push_store: std::sync::Arc::clone(&store),
            delivery: std::sync::Arc::clone(&delivery),
            presence: std::sync::Arc::clone(&presence),
            machine_identity: collaboration_service::MachineIdentity::new(
                collaboration_protocol::UuidIdentity::try_from(FIXTURE_SERVICE_ID.to_owned())
                    .expect("service id"),
                Some("mcp-message-test"),
            )
            .expect("machine identity"),
            clock: std::sync::Arc::new(collaboration_service::SystemSubscriptionClock),
        },
    );
    owner.start().await.expect("delivery owner starts");
    let identity = crate::api_test_harness::test_identity()
        .with_automation_store(store)
        .with_session_delivery(delivery)
        .with_subscription_delivery_service(owner.clone(), presence);
    (
        CollaborationMcpServer::for_application(
            collaboration_service::CollaborationApplication::new(identity),
            directory.to_owned(),
        ),
        owner,
    )
}

#[tokio::test]
async fn direct_message_tools_store_show_list_and_reply_through_the_application() {
    // Arrange
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (server, owner) = message_server(temporary.path()).await;
    let target = fixture_session("claude-local", "target-session");
    let sender = fixture_session("codex-local", "reported-sender");
    let request: collaboration_client::MessageSendRequest =
        serde_json::from_value(serde_json::json!({
            "target":target,
            "message":{"kind":"agent","sender":sender,"text":"reporting identity"},
            "delivery":"auto",
            "generationGuard":null
        }))
        .expect("message_send input");

    // Act
    let sent = server.message_send(super::super::Parameters(request)).await;
    let sent = sent.structured_content.expect("push result");
    let push_id = sent["pushId"]
        .as_str()
        .unwrap_or_else(|| panic!("stored push id: {sent}"))
        .to_owned();
    let inbox = server
        .message_inbox(super::super::Parameters(
            serde_json::from_value(serde_json::json!({"caller":target,"limit":20}))
                .expect("message_inbox input"),
        ))
        .await;
    let history = server
        .message_history(super::super::Parameters(
            serde_json::from_value(serde_json::json!({"caller":target,"with":sender,"limit":20}))
                .expect("message_history input"),
        ))
        .await;
    let shown = server
        .router_show(super::super::Parameters(
            serde_json::from_value(serde_json::json!({"caller":target,"reference":push_id}))
                .expect("router_show input"),
        ))
        .await;
    let reply = server
        .message_reply(super::super::Parameters(
            serde_json::from_value(serde_json::json!({
                "caller":target,
                "reference":format!("router://{FIXTURE_SERVICE_ID}/push/{push_id}"),
                "text":"reply by id"
            }))
            .expect("message_reply input"),
        ))
        .await;

    // Assert
    let success_fields = sent
        .as_object()
        .expect("push result object")
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        success_fields,
        BTreeSet::from([
            "deliveryState",
            "link",
            "pushId",
            "receipt",
            "target",
            "targetIdentity"
        ]),
        "a success keeps the published, untagged push result"
    );
    assert_eq!(
        sent["link"],
        format!("router://{FIXTURE_SERVICE_ID}/push/{push_id}")
    );
    assert_eq!(sent["target"], target);
    assert_eq!(sent["receipt"]["outcome"]["kind"], "peerMessageWritten");
    let inbox = inbox.structured_content.expect("inbox result");
    assert_eq!(inbox["records"][0]["pushId"], push_id.as_str());
    let history = history.structured_content.expect("history result");
    assert_eq!(history["records"][0]["pushId"], push_id.as_str());
    assert_ne!(shown.is_error, Some(true));
    let shown = shown.structured_content.expect("shown record");
    assert_eq!(shown["record"]["origin"]["session"], sender);
    assert!(
        shown["record"]["readAt"].is_string(),
        "a show by the target marks the direct message read"
    );
    assert_ne!(reply.is_error, Some(true));
    let reply = reply.structured_content.expect("reply result");
    assert_eq!(
        reply["target"], sender,
        "a reply goes to the referenced sender"
    );
    assert_eq!(reply["receipt"]["outcome"]["kind"], "peerMessageWritten");
    owner.shutdown().await;
}

#[tokio::test]
async fn router_show_by_an_unrelated_session_is_a_typed_not_permitted_error() {
    // Arrange
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (server, owner) = message_server(temporary.path()).await;
    let request: collaboration_client::MessageSendRequest =
        serde_json::from_value(serde_json::json!({
            "target":fixture_session("claude-local", "target-session"),
            "message":{"kind":"agent","sender":fixture_session("codex-local", "sender"),"text":"private"},
            "delivery":"auto",
            "generationGuard":null
        }))
        .expect("message_send input");
    let sent = server.message_send(super::super::Parameters(request)).await;
    let push_id = sent.structured_content.expect("push result")["pushId"].clone();
    let caller = fixture_session("cursor-local", "unrelated-session");

    // Act
    let result = server
        .router_show(super::super::Parameters(
            serde_json::from_value(serde_json::json!({"caller":caller,"reference":push_id}))
                .expect("router_show input"),
        ))
        .await;

    // Assert
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("typed read error");
    assert_eq!(structured["serviceKind"], "notPermitted");
    assert_eq!(structured["stage"], "inspect");
    assert_eq!(structured["effect"], "none");
    owner.shutdown().await;
}

#[tokio::test]
async fn native_sessions_list_routes_claude_to_provider_tool_without_changing_schema() {
    let server = CollaborationMcpServer::catalog_only();
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
    let server = CollaborationMcpServer::catalog_only();
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
        let result = super::super::message_receipt_result(push_result);
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

    let result = super::super::message_receipt_result(push_result);

    assert_ne!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured held success");
    assert_eq!(structured["deliveryState"], "held");
    assert_eq!(structured["link"], link);
    assert_eq!(structured["receipt"]["outcome"]["kind"], "notSubmitted");
}

#[test]
fn message_reply_is_by_reference_and_has_no_latest_sender_compatibility_fields() {
    let server = super::super::CollaborationMcpServer::catalog_only();
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

    let result = super::super::message_reply_receipt_result(reply_result);

    assert_ne!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured held success");
    assert_eq!(structured["deliveryState"], "held");
    assert_eq!(structured["link"], link);
    assert_eq!(structured["receipt"]["outcome"]["kind"], "notSubmitted");
}

#[test]
fn message_send_post_submission_failure_matches_advertised_output_schema() {
    let server = CollaborationMcpServer::catalog_only();
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
    let stored_but_unknown = collaboration_service::collaboration_application::MessageFailure {
        kind: collaboration_service::collaboration_application::MessageFailureKind::OutcomeUnknown,
        stage: collaboration_service::collaboration_application::MessageFailureStage::Store,
        message: "The message was stored; whether it was delivered is unknown.".to_owned(),
    };
    let result = super::super::failure_naming(
        &super::super::rejection_failure(&stored_but_unknown, OperationEffect::Unknown),
        "target",
        Some(target),
    );
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
    let server = CollaborationMcpServer::catalog_only();
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
    let result = super::super::message_receipt_result(push_result);
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
    let tool_result = super::super::thread_wait_outcome_unknown(&actor, &filter);
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

    let server = CollaborationMcpServer::catalog_only();
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
    let server = CollaborationMcpServer::catalog_only();
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
    let result = super::super::message_receipt_result(push_result);
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

#[tokio::test]
async fn message_rejections_before_submission_have_no_effect_and_keep_service_detail() {
    // Arrange
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (server, owner) = message_server(temporary.path()).await;
    let foreign_target = serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-0000000000ff","endpointId":"codex-local"},
        "sessionId":"elsewhere"
    });
    let send: collaboration_client::MessageSendRequest =
        serde_json::from_value(serde_json::json!({
            "target":foreign_target,
            "message":{"kind":"humanUser","text":"wrong Router"},
            "delivery":"auto",
            "generationGuard":null
        }))
        .expect("message_send input");
    let reply: collaboration_client::MessageReplyRequest =
        serde_json::from_value(serde_json::json!({
            "caller":fixture_session("codex-local", "target-session"),
            "reference":"018f1f12-3456-7abc-8def-0123456789ff",
            "text":"reply to nothing"
        }))
        .expect("message_reply input");

    // Act
    let sent = server.message_send(super::super::Parameters(send)).await;
    let replied = server.message_reply(super::super::Parameters(reply)).await;

    // Assert
    let sent = sent.structured_content.expect("send rejection");
    assert_eq!(sent["kind"], "rejected");
    assert_eq!(sent["serviceKind"], "wrongService");
    assert_eq!(sent["effect"], "none");
    assert!(sent["target"].is_null());
    assert_eq!(sent["code"], -32602);
    assert_eq!(sent["data"]["kind"], "wrongService");
    let replied = replied.structured_content.expect("reply rejection");
    assert_eq!(replied["serviceKind"], "notFound");
    assert_eq!(replied["effect"], "none");
    assert!(replied["caller"].is_null());
    owner.shutdown().await;
}

#[test]
fn a_message_stored_without_a_known_delivery_names_its_target_with_unknown_effect() {
    let target: collaboration_protocol::SessionRef =
        serde_json::from_value(fixture_session("codex-local", "message-thread")).expect("target");
    let stored_but_unknown = collaboration_service::collaboration_application::MessageFailure {
        kind: collaboration_service::collaboration_application::MessageFailureKind::OutcomeUnknown,
        stage: collaboration_service::collaboration_application::MessageFailureStage::Store,
        message: "The message was stored; whether it was delivered is unknown.".to_owned(),
    };

    let result =
        super::super::message_failure_result(&stored_but_unknown, "target", target.clone());

    let structured = result.structured_content.expect("structured failure");
    assert_eq!(structured["effect"], "unknown");
    assert_eq!(structured["target"], serde_json::json!(target));
    assert_eq!(structured["serviceKind"], "outcomeUnknown");
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
        "collaboration API protocol violation: connection closed after dispatch"
    );
}
