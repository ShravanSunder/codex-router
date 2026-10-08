#![allow(clippy::expect_used, clippy::panic)]
//! The client waits the requested server window plus a transport allowance, no longer.

use collaboration_client::CollaborationClient;
use collaboration_protocol::*;
use serde_json::{Value, json};
use std::time::Duration;

#[path = "support/scripted_api.rs"]
mod scripted_api;
use scripted_api::{SCRIPTED_SERVICE_EPOCH, SCRIPTED_SERVICE_ID, ScriptedApi};

/// A pending wait result for `operation_id`, as the Router answers an unsettled operation.
fn pending_wait_result(operation_id: &Value) -> Value {
    let operation: ConversationOperationSnapshot = serde_json::from_value(json!({
        "operationId":operation_id,
        "operation":"conversationPrompt",
        "binding":{"kind":"externalProvider","binding":{
            "endpoint":{"serviceId":SCRIPTED_SERVICE_ID,"endpointId":"claude-local"},
            "bindingId":"binding",
            "runtime":{"provider":"claudeCode","runtimeName":"fixture","runtimeVersion":null},
            "transport":"stdioAcp",
            "generation":{"serviceEpoch":SCRIPTED_SERVICE_EPOCH,"generation":1},
            "capabilities":[{"name":"prompt","status":"supported","evidence":"advertised"}]
        }},
        "target":null,"stage":"mayHaveDispatched","effect":"unknown",
        "reconciliation":"unresolved","admittedAt":"2026-09-22T00:00:00.000Z","terminalAt":null
    }))
    .expect("snapshot");
    serde_json::to_value(ConversationOperationWaitResult {
        operation,
        output: ConversationOperationWaitOutput::Pending,
    })
    .expect("wait result")
}

#[tokio::test(start_paused = true)]
async fn provider_wait_uses_requested_server_window_plus_transport_allowance() {
    let (api, mut calls) = ScriptedApi::start().await.expect("scripted API");
    let client = CollaborationClient::connect(api.directory(), "wait-proof", "1")
        .await
        .expect("connect");
    // The Router settles the wait 40 seconds after receiving it.
    let server = tokio::spawn(async move {
        let call = calls.next().await.expect("wait call");
        assert_eq!(call.tool, "conversation_operation_wait");
        assert_eq!(call.arguments["timeoutSeconds"], 60);
        tokio::time::sleep(Duration::from_secs(40)).await;
        let result = pending_wait_result(&call.arguments["operationId"]);
        call.succeed(result);
    });
    let operation_id = OperationId::generate();
    let result = client
        .wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
            operation_id: operation_id.clone(),
            timeout_seconds: PositiveSeconds::try_from(60).expect("timeout"),
        })
        .await
        .expect("40-second settlement remains within requested wait window");
    assert_eq!(result.operation.operation_id, operation_id);
    assert!(matches!(
        result.output,
        ConversationOperationWaitOutput::Pending
    ));
    server.await.expect("server");
}

#[tokio::test(start_paused = true)]
async fn provider_wait_reports_true_transport_timeout_after_allowance() {
    let (api, mut calls) = ScriptedApi::start().await.expect("scripted API");
    let client = CollaborationClient::connect(api.directory(), "timeout-proof", "1")
        .await
        .expect("connect");
    // The Router would answer only after 70 seconds: past the 60-second window and its
    // 5-second transport allowance.
    let server = tokio::spawn(async move {
        let call = calls.next().await.expect("wait call");
        tokio::time::sleep(Duration::from_secs(70)).await;
        let result = pending_wait_result(&call.arguments["operationId"]);
        call.succeed(result);
    });
    let started = tokio::time::Instant::now();
    let wait = client
        .wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
            operation_id: OperationId::generate(),
            timeout_seconds: PositiveSeconds::try_from(60).expect("timeout"),
        })
        .await;
    assert!(matches!(
        wait,
        Err(collaboration_client::ClientError::Timeout)
    ));
    assert_eq!(started.elapsed().as_secs(), 65);
    server.abort();
}
