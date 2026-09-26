#[path = "test_support/message_backend_fixture.rs"]
mod message_backend_fixture;
use collaboration_protocol::{DeliveryClientReceipt, MessageDelivery, NativeSendAcceptance};
use message_backend_fixture::{
    MessageScenario, NativeReply, NativeStep, exercise, outcome_data, read,
};
use serde_json::json;

#[tokio::test]
async fn explicit_queue_rejects_unloaded_without_resume_or_enqueue() {
    let (result, requests) = exercise(MessageScenario {
        delivery: MessageDelivery::Queue,
        steps: vec![read("notLoaded")],
    })
    .await
    .unwrap();
    let outcome = outcome_data(result).unwrap();
    assert_eq!(outcome["kind"], "notSubmitted");
    assert_eq!(outcome["reason"], "threadNotLoaded");
    assert_eq!(requests.len(), 1);
}
#[tokio::test]
async fn unload_after_loaded_admission_preserves_queue_acceptance_without_compensation() {
    // Arrange: native residency changes after the metadata check, before its queue receipt.
    let (result, requests) = exercise(MessageScenario {
        delivery: MessageDelivery::Queue,
        steps: vec![
            read("idle"),
            NativeStep {
                method: "thread/queue/add",
                reply: NativeReply::NotificationThenResult {
                    notification: json!({"method":"thread/status/changed","params":{
                        "threadId":"target","status":{"type":"notLoaded"}}}),
                    result: json!({"queuedSubmission":{"id":"accepted-while-unloaded"}}),
                },
            },
        ],
    })
    .await
    .unwrap();
    // Assert: a loaded-at-admission check is not falsely presented as a residency lease.
    let receipt = result.unwrap();
    let Some(DeliveryClientReceipt::CodexAppServer(native)) = receipt.client else {
        panic!("queued delivery lost its native receipt");
    };
    assert!(
        matches!(native.acceptance, NativeSendAcceptance::QueueAccepted { submission_id }
        if String::from(submission_id.clone()) == "accepted-while-unloaded")
    );
    assert!(matches!(
        native.resume_effect,
        collaboration_protocol::AcceptedResumeEffect::NotRequested
    ));
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]["params"]["clientUserMessageId"],
        serde_json::to_value(native.client_user_message_id).unwrap()
    );
    // The fixture additionally rejects any extra resume, start, queue deletion or replay.
}
#[tokio::test]
async fn auto_resume_preserves_effect_when_submission_rejected() {
    let (result, requests) = exercise(MessageScenario {
        delivery: MessageDelivery::Auto,
        steps: vec![
            read("notLoaded"),
            NativeStep {
                method: "thread/resume",
                reply: NativeReply::Result(json!({"thread":{"id":"target"}})),
            },
            NativeStep {
                method: "turn/start",
                reply: NativeReply::Reject,
            },
        ],
    })
    .await
    .unwrap();
    let outcome = outcome_data(result).unwrap();
    assert_eq!(outcome["kind"], "rejected");
    assert_eq!(outcome["reason"], "unknown");
    assert_eq!(outcome["nextAction"], "retryLater");
    assert_eq!(outcome["clientCode"], -32602);
    assert_eq!(requests.len(), 3);
    assert!(
        requests[2]["params"]["clientUserMessageId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert_eq!(requests[1]["params"]["excludeTurns"], true);
}
#[tokio::test]
async fn lost_resume_receipt_never_submits_input() {
    let (result, requests) = exercise(MessageScenario {
        delivery: MessageDelivery::Auto,
        steps: vec![
            read("notLoaded"),
            NativeStep {
                method: "thread/resume",
                reply: NativeReply::Disconnect,
            },
        ],
    })
    .await
    .unwrap();
    let outcome = outcome_data(result).unwrap();
    assert_eq!(outcome["kind"], "unknown");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["params"]["excludeTurns"], true);
}
#[tokio::test]
async fn auto_active_steers_exact_turn_without_loading_history() {
    let (result, requests) = exercise(MessageScenario { delivery:MessageDelivery::Auto, steps:vec![read("active"),
        NativeStep { method:"thread/turns/list", reply:NativeReply::Result(json!({"data":[{"id":"active-turn","status":"inProgress"}],"nextCursor":null,"backwardsCursor":null})) },
        NativeStep { method:"turn/steer", reply:NativeReply::Result(json!({"turnId":"active-turn"})) },
    ] }).await.unwrap();
    assert!(matches!(
        result.unwrap().client,
        Some(DeliveryClientReceipt::CodexAppServer(
            collaboration_protocol::NativeSendReceipt {
                acceptance: NativeSendAcceptance::SteerAccepted { .. },
                ..
            }
        ))
    ));
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[1]["params"],
        json!({
            "threadId": "target",
            "limit": 1,
            "sortDirection": "desc",
            "itemsView": "notLoaded"
        })
    );
    assert_eq!(requests[2]["params"]["expectedTurnId"], "active-turn");
}
#[tokio::test]
async fn explicit_steer_rejects_idle_without_starting_work() {
    let (result, requests) = exercise(MessageScenario {
        delivery: MessageDelivery::Steer,
        steps: vec![read("idle")],
    })
    .await
    .unwrap();
    let outcome = outcome_data(result).unwrap();
    assert_eq!(outcome["kind"], "notSubmitted");
    assert_eq!(outcome["reason"], "noActiveTurn");
    assert_eq!(requests.len(), 1);
}
