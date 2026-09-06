#[path = "test_support/message_backend_fixture.rs"]
mod message_backend_fixture;
use communication_protocol::{MessageDelivery, NativeSendAcceptance};
use message_backend_fixture::{
    MessageScenario, NativeReply, NativeStep, error_data, exercise, read,
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
    let error = error_data(result).unwrap();
    assert_eq!(error["kind"], "threadNotLoaded");
    assert_eq!(
        error["effects"],
        json!({"resume":"notRequested","submission":"notDispatched"})
    );
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
    assert!(
        matches!(receipt.acceptance, NativeSendAcceptance::QueueAccepted { submission_id }
        if String::from(submission_id.clone()) == "accepted-while-unloaded")
    );
    assert!(matches!(
        receipt.resume_effect,
        communication_protocol::AcceptedResumeEffect::NotRequested
    ));
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]["params"]["clientUserMessageId"],
        serde_json::to_value(receipt.client_user_message_id).unwrap()
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
    let error = error_data(result).unwrap();
    assert_eq!(error["kind"], "nativeRejected");
    assert_eq!(
        error["effects"],
        json!({"resume":"accepted","submission":"rejected"})
    );
    assert_eq!(
        error["clientUserMessageId"],
        requests[2]["params"]["clientUserMessageId"]
    );
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
    let error = error_data(result).unwrap();
    assert_eq!(
        error["effects"],
        json!({"resume":"unknown","submission":"notDispatched"})
    );
    assert_eq!(requests.len(), 2);
}
#[tokio::test]
async fn auto_active_steers_exact_observed_turn() {
    let (result, requests) = exercise(MessageScenario { delivery:MessageDelivery::Auto, steps:vec![read("active"),
        NativeStep { method:"thread/read", reply:NativeReply::Result(json!({"thread":{"id":"target","status":{"type":"active"},"turns":[{"id":"active-turn","status":"inProgress"}]}})) },
        NativeStep { method:"turn/steer", reply:NativeReply::Result(json!({"turnId":"active-turn"})) },
    ] }).await.unwrap();
    assert!(matches!(
        result.unwrap().acceptance,
        NativeSendAcceptance::SteerAccepted { .. }
    ));
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
    assert_eq!(error_data(result).unwrap()["kind"], "noActiveTurn");
    assert_eq!(requests.len(), 1);
}
