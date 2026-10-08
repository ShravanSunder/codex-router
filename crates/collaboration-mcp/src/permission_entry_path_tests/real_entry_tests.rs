use super::*;

#[tokio::test]
async fn cli_decision_response_loss_after_real_broker_effect_is_unknown_without_replay() {
    let fixture = ApprovalFixture::start_with_dropped_decision_reply(1, true, false).await;
    let request = fixture.begin_permission_request("cli-response-loss").await;
    let pending = fixture.pending_record().await;
    fixture.await_delivery(0).await;

    assert_eq!(
        run_cli_decision(
            fixture.service_directory(),
            &pending.request_id,
            &fixture.approver,
        )
        .await,
        5,
        "the actual CLI entry projects post-dispatch response loss as unknown",
    );
    assert_eq!(
        fixture.forwarded_decision_count(),
        1,
        "the SDK did not replay"
    );
    assert_eq!(
        request
            .await
            .expect("request join")
            .expect("broker request"),
        BrokeredApprovalOutcome::Selected {
            option_id: "native-accept".to_owned()
        },
        "the real broker applied the decision before its Control receipt was lost",
    );
    let approvals = fixture.broker.list(false).await.approvals;
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].request_id, pending.request_id);
    assert_eq!(approvals[0].decision, Some(ApprovalDecision::Allow));
    fixture.shutdown().await;
}

#[tokio::test]
async fn cli_command_entry_authorizes_real_broker_permission_by_actor_target_and_generation() {
    let fixture = ApprovalFixture::start(2).await;
    let request = fixture.begin_permission_request("cli").await;
    let pending = fixture.pending_record().await;
    fixture.await_delivery(0).await;
    assert_eq!(pending.requester, fixture.requester);
    assert_eq!(pending.approver, fixture.approver);
    assert_eq!(pending.generation, fixture.generation);
    assert!(
        pending.operation["params"]["toolCall"]["content"][0]["content"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Requested permissions:"))
    );

    let wrong_actor: SessionRef = serde_json::from_value(json!({
        "endpoint":fixture.approver.endpoint,"sessionId":"wrong-approver"
    }))
    .expect("wrong actor");
    assert_eq!(
        run_cli_decision(
            fixture.service_directory(),
            &pending.request_id,
            &wrong_actor
        )
        .await,
        4
    );
    assert_eq!(
        run_cli_decision(
            fixture.service_directory(),
            &pending.request_id,
            &fixture.approver
        )
        .await,
        0
    );
    assert_eq!(
        request
            .await
            .expect("request join")
            .expect("broker request"),
        BrokeredApprovalOutcome::Selected {
            option_id: "native-accept".to_owned()
        }
    );

    let mut stale_generation = fixture.generation.clone();
    stale_generation.generation = 6_u64.try_into().expect("stale generation");
    let stale_request = fixture
        .begin_permission_request_for_generation("cli-stale", stale_generation)
        .await;
    let stale = fixture.pending_record().await;
    fixture.await_delivery(1).await;
    assert_ne!(stale.generation, fixture.generation);
    assert_eq!(
        run_cli_decision(
            fixture.service_directory(),
            &stale.request_id,
            &fixture.approver
        )
        .await,
        4
    );
    assert_eq!(
        stale_request
            .await
            .expect("stale request join")
            .expect("stale broker request"),
        BrokeredApprovalOutcome::Cancelled
    );
    fixture.shutdown().await;
}

async fn mcp_call(
    client: &reqwest::Client,
    url: &str,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    let response = client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({
            "jsonrpc":"2.0", "id":id, "method":"tools/call",
            "params":{"name":name,"arguments":arguments}
        }))
        .send()
        .await
        .expect("MCP call");
    let status = response.status();
    let body = response.text().await.expect("MCP response body");
    let payload = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|data| !data.is_empty())
        .unwrap_or(&body);
    serde_json::from_str(payload).unwrap_or_else(|error| {
        panic!("MCP response JSON: {error}; status={status}; body={body:?}")
    })
}

#[tokio::test]
async fn provider_session_approval_uses_legacy_default_list_and_safe_decision() {
    let fixture = ApprovalFixture::start_typed().await;
    let requester: message_board::SessionRef =
        serde_json::from_value(serde_json::to_value(&fixture.requester).expect("requester JSON"))
            .expect("board requester");
    let approver: message_board::SessionRef =
        serde_json::from_value(serde_json::to_value(&fixture.approver).expect("approver JSON"))
            .expect("board approver");
    let request = serde_json::from_value(json!({
        "requestId":"provider-legacy-approval", "title":"Run command",
        "options":[
            {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}},
            {"optionId":"allow-always","label":"Always allow","choice":{"effect":"allow","scope":{"persistent":{"where_stored":"provider settings"}}}}
        ]
    })).expect("provider approval");
    let receiver = fixture
        .broker
        .request_typed_approval(
            requester,
            message_board::Identity::Session { session: approver },
            request,
            CancellationToken::new(),
            CancellationToken::new(),
            Some(collaboration_service::TypedApprovalLegacyContext {
                operation_id: collaboration_protocol::OperationId::generate(),
                target: fixture.requester.clone(),
                generation: fixture.generation.clone(),
                requested_by: fixture.requester.clone().into(),
            }),
        )
        .await
        .expect("provider approval pending");
    let listener = ServedApi::tcp(&api_config(
        fixture.application.clone(),
        fixture.service_directory(),
    ))
    .await;
    let client = reqwest::Client::new();
    let listed = mcp_call(
        &client,
        &listener.url(),
        2,
        "approval_list",
        json!({"pending":true}),
    )
    .await;
    let row = &listed["result"]["structuredContent"]["approvals"][0];
    assert_eq!(row["requestId"], "provider-legacy-approval");
    assert_eq!(row["approver"], json!(fixture.approver));
    assert_eq!(row["generation"], json!(fixture.generation));
    assert!(row["expiresAt"].is_string());
    assert!(row["operation"]["target"].is_object());
    assert!(row.get("optionsOrigin").is_none());
    let decided = mcp_call(
        &client,
        &listener.url(),
        3,
        "approval_decide",
        json!({"requestId":"provider-legacy-approval",
            "decision":"allow","actor":fixture.approver}),
    )
    .await;
    assert_eq!(decided["result"]["isError"], false);
    assert_eq!(decided["result"]["structuredContent"]["state"], "decided");
    assert!(matches!(receiver.await.expect("agent resolution"),
        collaboration_service::TypedApprovalResolution::Selected(selected)
            if selected.option_id.as_str() == "allow-once"));
    listener.stop().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn streamable_http_entry_authorizes_real_broker_permission_by_actor_target_and_generation() {
    let fixture = ApprovalFixture::start(1).await;
    let listener = ServedApi::tcp(&api_config(
        fixture.application.clone(),
        fixture.service_directory(),
    ))
    .await;
    let client = reqwest::Client::new();

    let request = fixture.begin_permission_request("mcp").await;
    let pending = fixture.pending_record().await;
    fixture.await_delivery(0).await;
    let list = mcp_call(
        &client,
        &listener.url(),
        2,
        "approval_list",
        json!({"pending":true}),
    )
    .await;
    let listed = &list["result"]["structuredContent"]["approvals"][0];
    assert_eq!(listed["requestId"], pending.request_id);
    assert_eq!(listed["requester"], serde_json::json!(fixture.requester));
    assert_eq!(listed["approver"], serde_json::json!(fixture.approver));
    assert!(listed.get("optionsOrigin").is_none());
    let detailed = mcp_call(
        &client,
        &listener.url(),
        20,
        "approval_list",
        json!({"pending":true,"includeOptions":true}),
    )
    .await;
    let listed = &detailed["result"]["structuredContent"]["approvals"][0];
    assert_eq!(
        listed["approver"],
        json!({"kind":"session","session":fixture.approver})
    );
    let options = listed["options"].as_array().expect("offered options");
    assert!(
        options
            .iter()
            .any(|option| option["optionId"] == "native-accept")
    );
    assert!(
        options
            .iter()
            .any(|option| option["optionId"] == "native-accept-session")
    );
    assert!(
        options
            .iter()
            .any(|option| option["optionId"] == "native-decline")
    );

    let wrong_actor: SessionRef = serde_json::from_value(json!({
        "endpoint":fixture.approver.endpoint,"sessionId":"wrong-approver"
    }))
    .expect("wrong actor");
    let rejected = mcp_call(
        &client,
        &listener.url(),
        3,
        "approval_decide",
        json!({
            "requestId":pending.request_id,"decision":"allow",
            "actor":wrong_actor
        }),
    )
    .await;
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(
        rejected["result"]["structuredContent"]["data"]["kind"],
        "wrongActor"
    );
    let accepted = mcp_call(
        &client,
        &listener.url(),
        4,
        "approval_decide",
        json!({
            "requestId":pending.request_id,"decision":"allowForSession",
            "actor":fixture.approver
        }),
    )
    .await;
    assert_eq!(accepted["result"]["isError"], false);
    assert_eq!(
        accepted["result"]["structuredContent"]["decision"],
        serde_json::json!(ApprovalDecision::AllowForSession)
    );
    assert_eq!(
        accepted["result"]["structuredContent"]["scope"],
        "nativeSession"
    );
    assert_eq!(
        request
            .await
            .expect("request join")
            .expect("broker request"),
        BrokeredApprovalOutcome::Selected {
            option_id: "native-accept-session".to_owned()
        }
    );
    listener.stop().await;
    fixture.shutdown().await;
}

// R18: MCP presents the typed form and forwards the Approver's answer to the
// waiting broker request through the same public Control path as the CLI.
#[tokio::test]
async fn streamable_http_question_form_reaches_the_waiting_agent() {
    let fixture = ApprovalFixture::start(1).await;
    let requester =
        serde_json::from_value(serde_json::to_value(&fixture.requester).expect("requester JSON"))
            .expect("typed requester");
    let approver = serde_json::from_value(json!({"kind":"session","session":fixture.approver}))
        .expect("typed approver");
    let question = serde_json::from_value(json!({
        "requestId":"mcp-question", "prompt":"Choose settings", "fields":[
            {"kind":"number","fieldId":"count","label":"Count","description":null,"required":true},
            {"kind":"boolean","fieldId":"dryRun","label":"Dry run","description":null,"required":true},
            {"kind":"singleChoice","fieldId":"color","label":"Color","description":null,"required":true,"options":[{"optionId":"red","label":"Red"},{"optionId":"blue","label":"Blue"}]}
        ]
    })).expect("question");
    let agent_reply = fixture
        .broker
        .request_question(requester, approver, question, None)
        .await
        .expect("pending question");
    fixture.await_delivery(0).await;
    let listener = ServedApi::tcp(&api_config(
        fixture.application.clone(),
        fixture.service_directory(),
    ))
    .await;
    let client = reqwest::Client::new();
    let listed = mcp_call(
        &client,
        &listener.url(),
        2,
        "question_list",
        json!({"pending":true}),
    )
    .await;
    let question = &listed["result"]["structuredContent"]["questions"][0];
    assert_eq!(question["prompt"], "Choose settings");
    assert_eq!(question["fields"][0]["kind"], "number");
    let answered = mcp_call(
        &client,
        &listener.url(),
        3,
        "question_answer",
        json!({
            "requestId":"mcp-question", "actor":fixture.approver,
            "response":{"action":"answered","content":{"count":3,"dryRun":true,"color":{"selectedOptionIds":["blue"]}}}
        }),
    )
    .await;
    assert_eq!(answered["result"]["isError"], false);
    assert_eq!(answered["result"]["structuredContent"]["state"], "answered");
    let response = agent_reply.await.expect("agent response");
    assert_eq!(
        serde_json::to_value(response).expect("response JSON"),
        json!({
            "action":"answered","content":{"count":3,"dryRun":true,"color":{"selectedOptionIds":["blue"]}}
        })
    );
    listener.stop().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn stateless_http_create_reports_manifest_preflight_without_creation_uncertainty() {
    // The listener is real, but its service directory deliberately has no manifest. This reaches the MCP adapter's actual create entry point
    // while proving that discovery failure occurred before ACP setup/dispatch.
    let directory = tempfile::tempdir_in("/tmp").expect("private service directory");
    let listener = ServedApi::tcp(&api_config(
        collaboration_service::CollaborationApplication::new(
            crate::api_test_harness::test_identity(),
        ),
        directory.path(),
    ))
    .await;
    let client = reqwest::Client::new();

    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"codex-local"});
    let local_identity = json!({"endpoint":endpoint,"sessionId":"mcp-caller"});
    let foreign_identity = json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000099","endpointId":"codex-local"},
        "sessionId":"foreign-caller"
    });
    for (id, created_by, approver) in [
        (2, Value::Null, local_identity.clone()),
        (3, foreign_identity, local_identity.clone()),
    ] {
        let missing_creator = created_by.is_null();
        let response = mcp_call(
            &client,
            &listener.url(),
            id,
            "conversation_create",
            json!({
                "operationId":collaboration_protocol::OperationId::generate(),
                "endpoint":endpoint,"workingDirectory":"/tmp", "fork":null,
                "model":"gpt-5.6-sol", "effort":"low", "access":"workspace-write",
                "createdBy":created_by, "approver":approver, "rootMessageId":null
            }),
        )
        .await;
        if missing_creator {
            assert_eq!(response["result"]["isError"], true);
            assert!(
                response["result"]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|message| message.contains("expected struct SessionRef")),
                "{response}"
            );
            assert!(response["result"].get("structuredContent").is_none());
            continue;
        }
        let failure = &response["result"]["structuredContent"];
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(failure["target"], Value::Null);
        assert_eq!(failure["effect"], "none", "{response}");
        assert_eq!(failure["stage"], "validation");
    }

    let response = mcp_call(
        &client,
        &listener.url(),
        4,
        "conversation_create",
        json!({
            "operationId":collaboration_protocol::OperationId::generate(),
            "endpoint":endpoint,"workingDirectory":"/tmp", "fork":null,
            "model":"gpt-5.6-sol", "effort":"low", "access":"workspace-write",
            "createdBy":local_identity, "approver":local_identity, "rootMessageId":null
        }),
    )
    .await;
    let failure = &response["result"]["structuredContent"];
    assert_eq!(failure["effect"], "none");
    assert_eq!(failure["stage"], "manifest-read");

    let wake_response = mcp_call(
        &client,
        &listener.url(),
        5,
        "wake_wait_until_first_fire",
        json!({"wakeupId":"01985b1e-8d90-7fff-8000-000000000099"}),
    )
    .await;
    let wake_failure = &wake_response["result"]["structuredContent"];
    assert_eq!(wake_failure["kind"], "waitUnavailable", "{wake_response}");
    assert_eq!(wake_failure["effect"], "none");
    listener.stop().await;
}
