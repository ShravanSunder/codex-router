use super::{MessageSendError, MessageSendRequest, PublicMessageContent};
use crate::scripted_api::{ScriptedApi, ScriptedCalls};
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{
    CodexGeneration, EndpointId, EndpointRef, MessageDelivery, MessageText, SessionId, SessionRef,
    UuidIdentity,
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::task::JoinHandle;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const PUSH_ID: &str = "019f0000-0000-7000-8000-000000000101";
const OTHER_PUSH_ID: &str = "019f0000-0000-7000-8000-000000000102";

async fn connected_client() -> (ScriptedApi, CollaborationClient, ScriptedCalls) {
    let (api, calls) = ScriptedApi::start().await.expect("scripted API");
    let client = CollaborationClient::connect(api.directory(), "message-test", "1")
        .await
        .expect("connect");
    (api, client, calls)
}

/// A refused, unsent or unknown delivery as the API publishes it: a tool error carrying the
/// stored push and its receipt.
fn published_delivery_failure(mut receipt: Value, kind: &str, effect: &str) -> Value {
    if let Some(fields) = receipt.as_object_mut() {
        fields.insert("mcpResult".to_owned(), json!("error"));
        fields.insert("kind".to_owned(), json!(kind));
        fields.insert("message".to_owned(), json!("fixture delivery failure"));
        fields.insert("effect".to_owned(), json!(effect));
    }
    receipt
}

#[tokio::test]
async fn wrong_service_is_rejected_before_endpoint_discovery_or_submission() {
    let (_api, client, mut calls) = connected_client().await;
    let request = MessageSendRequest {
        target: SessionRef {
            endpoint: EndpointRef {
                service_id: UuidIdentity::try_from(
                    "00000000-0000-4000-8000-000000000099".to_owned(),
                )
                .expect("service id"),
                endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("endpoint id"),
            },
            session_id: SessionId::try_from("target".to_owned()).expect("session id"),
        },
        message: PublicMessageContent::HumanUser {
            text: MessageText::try_from("hello".to_owned()).expect("message text"),
        },
        delivery: MessageDelivery::Auto,
        generation_guard: None,
    };
    let error = client
        .send_message(request)
        .await
        .expect_err("foreign service");
    assert!(matches!(
        error,
        MessageSendError::Preparation(source)
            if matches!(source.as_ref(), ClientError::InvalidRequest(
                "message target belongs to another service"
            ))
    ));
    assert!(calls.none_within(Duration::from_millis(50)).await);
}

#[tokio::test]
async fn strict_generation_guard_reaches_the_service_and_reports_known_none() {
    let (_api, client, mut calls) = connected_client().await;
    let peer = tokio::spawn(async move {
        let sent = calls.next().await.expect("message call");
        assert_eq!(sent.tool, "message_send");
        assert_eq!(sent.arguments["generationGuard"]["generation"], 1);
        sent.fail(published_delivery_failure(
            json!({
                "pushId":PUSH_ID,
                "link":format!("router://{SERVICE_ID}/push/{PUSH_ID}"),
                "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},"sessionId":"target"},
                "targetIdentity":"✳️ Codex target",
                "deliveryState":"rejected",
                "receipt":{
                    "outcome":{"kind":"notSubmitted","retryable":false,"reason":"staleGeneration"},
                    "reachability":"codexAppServer","client":null
                }
            }),
            "notSubmitted",
            "none",
        ));
    });
    let request: MessageSendRequest = serde_json::from_value(json!({
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"target"},
        "message":{"kind":"humanUser","text":"hello"},"delivery":"auto",
        "generationGuard":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1}
    }))
    .expect("message request");
    let receipt = client
        .send_message(request)
        .await
        .expect("stale guard is an E5 result");
    assert!(matches!(
        receipt.receipt.outcome,
        collaboration_protocol::DeliveryOutcome::NotSubmitted { retryable: false, reason }
            if reason == "staleGeneration"
    ));
    peer.await.expect("join peer");
}

#[tokio::test]
async fn unpinned_send_uses_service_route_without_native_catalog_precheck() {
    let (_api, client, mut calls) = connected_client().await;
    let peer = tokio::spawn(async move {
        let sent = calls.next().await.expect("message call");
        assert_eq!(sent.tool, "message_send");
        assert!(sent.arguments["generationGuard"].is_null());
        sent.succeed(json!({
            "pushId":PUSH_ID,
            "link":format!("router://{SERVICE_ID}/push/{PUSH_ID}"),
            "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},"sessionId":"target"},
            "targetIdentity":"✳️ Codex target",
            "deliveryState":"held",
            "receipt":{
                "outcome":{"kind":"notSubmitted","retryable":true,"reason":"provider starting"},
                "reachability":null,"client":null
            }
        }));
        // Only the message call reaches the API: no native catalog precheck.
        assert!(calls.none_within(Duration::from_millis(50)).await);
    });
    let request = fixture_request(None);
    assert!(matches!(
        client.send_message(request).await,
        Ok(collaboration_protocol::PushMessageSendResult {
            receipt: collaboration_protocol::DeliveryReceipt {
                outcome: collaboration_protocol::DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    ..
                },
                reachability: None,
                client: None
            },
            ..
        })
    ));
    peer.await.expect("join peer");
}

#[tokio::test]
async fn unavailable_after_message_submission_keeps_outcome_unknown() {
    let (_api, client, mut calls) = connected_client().await;
    let peer = tokio::spawn(async move {
        let sent = calls.next().await.expect("message call");
        assert_eq!(sent.tool, "message_send");
        let failure = crate::operation_failure_from_client_error(
            ClientError::Rejected {
                code: -32050,
                data: Some(
                    json!({"kind":"unavailable","stage":"start","message":"Native backend unavailable"}),
                ),
            },
            crate::OperationEffect::Unknown,
        );
        let mut failure = serde_json::to_value(failure).expect("published failure");
        failure["mcpResult"] = json!("error");
        sent.fail(failure);
    });
    let request = fixture_request(None);
    let expected_target = request.target.clone();
    let error = client
        .send_message(request)
        .await
        .expect_err("post-submission unavailable may hide an accepted delivery");
    assert!(matches!(
        error,
        MessageSendError::Submission { target, source }
            if target == expected_target
                && matches!(source.as_ref(), ClientError::Rejected { data: Some(data), .. }
                    if data["kind"] == "unavailable")
    ));
    peer.await.expect("join peer");
}

#[tokio::test]
async fn response_loss_after_message_dispatch_is_submission_failure() {
    let (_api, client, mut calls) = connected_client().await;
    let peer = tokio::spawn(async move {
        let send = calls.next().await.expect("message call");
        assert_eq!(send.tool, "message_send");
        send.lose_response();
    });
    let error = client
        .send_message(fixture_request(None))
        .await
        .expect_err("response loss after dispatch must fail");
    let (failure, target) = error.into_operation_failure_and_target();
    assert_eq!(failure.effect, crate::OperationEffect::Unknown);
    assert_eq!(target, Some(fixture_request(None).target));
    peer.await.expect("join peer");
}

#[tokio::test]
async fn successful_push_response_must_name_the_requested_target() {
    let request = fixture_request(None);
    let mut response = push_result(&request.target, PUSH_ID, PUSH_ID);
    response["target"]["sessionId"] = json!("different-target");
    let (_api, client, peer) = client_with_send_result(response).await;

    let error = client
        .send_message(request)
        .await
        .expect_err("a successful receipt for another target is inconsistent");
    assert_protocol_inconsistency(error);

    peer.await.expect("join peer");
}

#[tokio::test]
async fn successful_push_response_link_must_match_its_push_id() {
    let request = fixture_request(None);
    let response = push_result(&request.target, PUSH_ID, OTHER_PUSH_ID);
    let (_api, client, peer) = client_with_send_result(response).await;

    let error = client
        .send_message(request)
        .await
        .expect_err("a link to another push is inconsistent");
    assert_protocol_inconsistency(error);

    peer.await.expect("join peer");
}

#[tokio::test]
async fn native_push_receipt_matches_target_generation_mode_and_push_id() {
    let expected_generation = generation(1);
    let request = fixture_request(Some(expected_generation.clone()));
    let response = native_push_result(
        &request.target,
        &request.target,
        &expected_generation,
        PUSH_ID,
        PUSH_ID,
        "started",
    );
    let (_api, client, peer) = client_with_send_result(response).await;

    let result = client
        .send_message(request)
        .await
        .expect("matching native delivery receipt");
    assert_eq!(result.push_id.as_str(), PUSH_ID);
    peer.await.expect("join peer");

    let request = fixture_request(Some(expected_generation.clone()));
    let other_target = session_ref("different-target");
    let response = native_push_result(
        &request.target,
        &other_target,
        &expected_generation,
        PUSH_ID,
        PUSH_ID,
        "started",
    );
    assert_inconsistent_response(request, response).await;

    let request = fixture_request(Some(expected_generation.clone()));
    let response = native_push_result(
        &request.target,
        &request.target,
        &generation(2),
        PUSH_ID,
        PUSH_ID,
        "started",
    );
    assert_inconsistent_response(request, response).await;

    let mut request = fixture_request(Some(expected_generation.clone()));
    request.delivery = MessageDelivery::Queue;
    let response = native_push_result(
        &request.target,
        &request.target,
        &expected_generation,
        PUSH_ID,
        PUSH_ID,
        "started",
    );
    assert_inconsistent_response(request, response).await;

    let request = fixture_request(Some(expected_generation.clone()));
    let response = native_push_result(
        &request.target,
        &request.target,
        &expected_generation,
        PUSH_ID,
        OTHER_PUSH_ID,
        "started",
    );
    assert_inconsistent_response(request, response).await;
}

fn fixture_request(
    generation_guard: Option<collaboration_protocol::CodexGeneration>,
) -> MessageSendRequest {
    serde_json::from_value(json!({
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"target"},
        "message":{"kind":"humanUser","text":"hello"},"delivery":"auto",
        "generationGuard":generation_guard
    }))
    .expect("message request")
}

async fn client_with_send_result(
    result: Value,
) -> (ScriptedApi, CollaborationClient, JoinHandle<()>) {
    let (api, client, mut calls) = connected_client().await;
    let peer = tokio::spawn(async move {
        let send = calls.next().await.expect("message send call");
        assert_eq!(send.tool, "message_send");
        send.succeed(result);
    });
    (api, client, peer)
}

fn push_result(target: &SessionRef, push_id: &str, link_push_id: &str) -> Value {
    json!({
        "pushId":push_id,
        "link":format!("router://{SERVICE_ID}/push/{link_push_id}"),
        "target":target,
        "targetIdentity":"✳️ Codex target",
        "deliveryState":"delivered",
        "receipt":{
            "outcome":{"kind":"peerMessageWritten"},
            "reachability":"claudeCodePeer",
            "client":{"kind":"claudeCodePeer"}
        }
    })
}

fn native_push_result(
    target: &SessionRef,
    native_target: &SessionRef,
    generation: &CodexGeneration,
    push_id: &str,
    client_user_message_id: &str,
    outcome: &str,
) -> Value {
    json!({
        "pushId":push_id,
        "link":format!("router://{SERVICE_ID}/push/{push_id}"),
        "target":target,
        "targetIdentity":"✳️ Codex target",
        "deliveryState":"delivered",
        "receipt":{
            "outcome":{"kind":outcome},
            "reachability":"codexAppServer",
            "client":{
                "kind":"codexAppServer",
                "target":native_target,
                "generation":generation,
                "inputKind":"agent",
                "representation":"declaredAgentText",
                "clientUserMessageId":client_user_message_id,
                "resumeEffect":"notRequested",
                "acceptance":{
                    "kind":"nativeInputAccepted",
                    "operation":"turnStart",
                    "disposition":"startedOrSteered",
                    "turnId":"turn-1"
                }
            }
        }
    })
}

fn generation(generation: u64) -> CodexGeneration {
    serde_json::from_value(json!({
        "serviceEpoch":"00000000-0000-4000-8000-000000000002",
        "generation":generation
    }))
    .expect("Codex generation")
}

fn session_ref(session_id: &str) -> SessionRef {
    serde_json::from_value(json!({
        "endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},
        "sessionId":session_id
    }))
    .expect("session reference")
}

fn assert_protocol_inconsistency(error: MessageSendError) {
    assert!(matches!(
        error,
        MessageSendError::Submission { source, .. }
            if matches!(source.as_ref(), ClientError::Protocol(
                "inconsistent message receipt; acceptance unknown"
            ))
    ));
}

async fn assert_inconsistent_response(request: MessageSendRequest, response: Value) {
    let (_api, client, peer) = client_with_send_result(response).await;
    let error = client
        .send_message(request)
        .await
        .expect_err("inconsistent delivery data must fail closed");
    assert_protocol_inconsistency(error);
    peer.await.expect("join peer");
}

fn explicit_mode_receipt(request: &MessageSendRequest, resume: &str) -> Value {
    let mut response = native_push_result(
        &request.target,
        &request.target,
        request
            .generation_guard
            .as_ref()
            .expect("fixture generation"),
        PUSH_ID,
        PUSH_ID,
        "queued",
    );
    let (outcome, acceptance) = match request.delivery {
        MessageDelivery::Queue => (
            "queued",
            json!({"kind":"queueAccepted","submissionId":"queue-1"}),
        ),
        MessageDelivery::Steer => ("steered", json!({"kind":"steerAccepted","turnId":"turn-1"})),
        MessageDelivery::Auto => panic!("explicit mode required"),
    };
    response["receipt"]["outcome"]["kind"] = json!(outcome);
    response["receipt"]["client"]["acceptance"] = acceptance;
    response["receipt"]["client"]["resumeEffect"] = json!(resume);
    response
}

/// Completion is the no-replay barrier; a second queued API call wins the select.
async fn single_send_result(
    request: MessageSendRequest,
    response: Value,
) -> Result<collaboration_protocol::PushMessageSendResult, MessageSendError> {
    let (_api, client, mut calls) = connected_client().await;
    let (completed, completion) = tokio::sync::oneshot::channel::<()>();
    let peer = tokio::spawn(async move {
        let call = calls.next().await.expect("one message send");
        assert_eq!(call.tool, "message_send");
        call.succeed(response);
        tokio::select! {
            biased;
            extra = calls.next() => panic!("unexpected replay: {}", extra.is_ok()),
            finished = completion => assert!(finished.is_ok()),
        }
    });
    let result = client.send_message(request).await;
    let _finished = completed.send(());
    peer.await.expect("single-send peer");
    result
}

#[tokio::test]
async fn explicit_queue_and_steer_accept_truthful_resume_effects() {
    for delivery in [MessageDelivery::Queue, MessageDelivery::Steer] {
        for resume in ["accepted", "notRequested"] {
            let mut request = fixture_request(Some(generation(1)));
            request.delivery = delivery;
            let response = explicit_mode_receipt(&request, resume);
            let receipt = single_send_result(request, response)
                .await
                .expect("truthful receipt");
            assert_eq!(receipt.push_id.as_str(), PUSH_ID);
            let Some(collaboration_protocol::DeliveryClientReceipt::CodexAppServer(native)) =
                receipt.receipt.client
            else {
                panic!("native receipt missing");
            };
            assert_eq!(
                serde_json::to_value(native.resume_effect).expect("effect"),
                json!(resume)
            );
        }
    }
}

#[tokio::test]
async fn explicit_native_receipt_guards_remain_strict_without_replay() {
    #[derive(Clone, Copy, Debug)]
    enum NativeReceiptDefect {
        OuterTarget,
        NativeTarget,
        Generation,
        Link,
        Correlation,
        InputKind,
        Representation,
        Outcome,
        Acceptance,
        ResumeWire,
    }
    for delivery in [MessageDelivery::Queue, MessageDelivery::Steer] {
        for defect in [
            NativeReceiptDefect::OuterTarget,
            NativeReceiptDefect::NativeTarget,
            NativeReceiptDefect::Generation,
            NativeReceiptDefect::Link,
            NativeReceiptDefect::Correlation,
            NativeReceiptDefect::InputKind,
            NativeReceiptDefect::Representation,
            NativeReceiptDefect::Outcome,
            NativeReceiptDefect::Acceptance,
            NativeReceiptDefect::ResumeWire,
        ] {
            let mut request = fixture_request(Some(generation(1)));
            request.delivery = delivery;
            let expected_target = request.target.clone();
            let mut response = explicit_mode_receipt(&request, "accepted");
            match defect {
                NativeReceiptDefect::OuterTarget => {
                    response["target"]["sessionId"] = json!("other")
                }
                NativeReceiptDefect::NativeTarget => {
                    response["receipt"]["client"]["target"]["sessionId"] = json!("other")
                }
                NativeReceiptDefect::Generation => {
                    response["receipt"]["client"]["generation"] = json!(generation(2))
                }
                NativeReceiptDefect::Link => {
                    response["link"] = json!(format!("router://{SERVICE_ID}/push/{OTHER_PUSH_ID}"))
                }
                NativeReceiptDefect::Correlation => {
                    response["receipt"]["client"]["clientUserMessageId"] = json!(OTHER_PUSH_ID)
                }
                NativeReceiptDefect::InputKind => {
                    response["receipt"]["client"]["inputKind"] = json!("humanUser")
                }
                NativeReceiptDefect::Representation => {
                    response["receipt"]["client"]["representation"] = json!("humanUserText")
                }
                NativeReceiptDefect::Outcome => {
                    response["receipt"]["outcome"]["kind"] = json!("started")
                }
                NativeReceiptDefect::Acceptance => {
                    response["receipt"]["client"]["acceptance"] = json!({
                    "kind":"nativeInputAccepted","operation":"turnStart",
                    "disposition":"startedOrSteered","turnId":"turn-1"})
                }
                NativeReceiptDefect::ResumeWire => {
                    response["receipt"]["client"]["resumeEffect"] = json!("invented")
                }
            }
            let error = single_send_result(request, response)
                .await
                .expect_err("corrupt receipt");
            let (failure, target) = error.into_operation_failure_and_target();
            assert_eq!(
                failure.effect,
                crate::OperationEffect::Unknown,
                "{defect:?}"
            );
            assert_eq!(
                failure.kind,
                collaboration_protocol::OperationFailureKind::ProtocolViolation
            );
            assert_eq!(target, Some(expected_target));
        }
    }
}
