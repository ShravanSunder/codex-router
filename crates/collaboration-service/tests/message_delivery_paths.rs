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
    assert_eq!(outcome["reason"], "notLoaded");
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
async fn auto_loaded_start_rejection_preserves_effect() {
    let (result, requests) = exercise(MessageScenario {
        delivery: MessageDelivery::Auto,
        steps: vec![
            read("idle"),
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
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]["params"]["clientUserMessageId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
}

#[tokio::test]
async fn foreign_writer_start_returns_actionable_typed_rejection_without_retry() {
    let (result, requests) = exercise(MessageScenario {
        delivery: MessageDelivery::Auto,
        steps: vec![
            read("idle"),
            NativeStep {
                method: "turn/start",
                reply: NativeReply::RejectWith {
                    code: -32600,
                    message: "thread target already has an active writer",
                },
            },
        ],
    })
    .await
    .unwrap();
    let outcome = outcome_data(result).unwrap();
    assert_eq!(outcome["kind"], "rejected");
    assert_eq!(outcome["reason"], "heldByAnotherClient");
    assert_eq!(outcome["nextAction"], "messageFromHoldingCodexClient");
    assert_eq!(
        outcome["detail"],
        "Message it from the Codex client that holds it."
    );
    assert_eq!(outcome["clientCode"], -32600);
    assert_eq!(requests.len(), 2, "no retry after writer refusal");
}
#[tokio::test]
async fn lost_start_receipt_is_unknown_and_never_replayed() {
    let (result, requests) = exercise(MessageScenario {
        delivery: MessageDelivery::Auto,
        steps: vec![
            read("idle"),
            NativeStep {
                method: "turn/start",
                reply: NativeReply::Disconnect,
            },
        ],
    })
    .await
    .unwrap();
    let outcome = outcome_data(result).unwrap();
    assert_eq!(outcome["kind"], "unknown");
    assert_eq!(requests.len(), 2);
    assert!(requests[1]["params"]["clientUserMessageId"].is_string());
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

type FixtureError = Box<dyn std::error::Error + Send + Sync>;

struct RecordedReceiptWitness {
    observation: message_backend_fixture::RecordedMessageObservation,
    stored: collaboration_protocol::PushRecord,
    persisted: collaboration_protocol::DeliveryReceipt,
    native: collaboration_protocol::NativeSendReceipt,
}

async fn recorded_loaded_witness(
    mode: MessageDelivery,
) -> Result<RecordedReceiptWitness, FixtureError> {
    let mut observation = message_backend_fixture::exercise_recorded_loaded(mode).await?;
    let stored = observation
        .stored
        .take()
        .ok_or("native correlation lost the stored push")?;
    let persisted = stored
        .last_outcome
        .clone()
        .ok_or("accepted delivery was not persisted")?;
    let native = match persisted.client.as_ref() {
        Some(DeliveryClientReceipt::CodexAppServer(native)) => native.clone(),
        _ => return Err("stored native receipt missing".into()),
    };
    Ok(RecordedReceiptWitness {
        observation,
        stored,
        persisted,
        native,
    })
}

#[tokio::test]
async fn recorded_loaded_queue_receipt_reaches_public_client() {
    let witness = recorded_loaded_witness(MessageDelivery::Queue)
        .await
        .expect("real fixture/store/native witness");
    assert_recorded_loaded_receipt(&witness, MessageDelivery::Queue);
    let public = witness
        .observation
        .result
        .expect("public client discarded truthful recorded-policy acceptance");
    assert_eq!(public.push_id, witness.stored.push_id);
    assert_eq!(public.target, witness.observation.target);
    assert_eq!(
        public.link,
        format!(
            "router://00000000-0000-4000-8000-000000000001/push/{}",
            public.push_id.as_str()
        )
    );
    assert_eq!(
        serde_json::to_value(public.receipt).expect("public receipt"),
        serde_json::to_value(witness.persisted).expect("stored receipt")
    );
}

#[tokio::test]
async fn recorded_loaded_steer_receipt_reaches_public_client() {
    let witness = recorded_loaded_witness(MessageDelivery::Steer)
        .await
        .expect("real fixture/store/native witness");
    assert_recorded_loaded_receipt(&witness, MessageDelivery::Steer);
    let public = witness
        .observation
        .result
        .expect("public client discarded truthful recorded-policy acceptance");
    assert_eq!(public.push_id, witness.stored.push_id);
    assert_eq!(public.target, witness.observation.target);
    assert_eq!(
        public.link,
        format!(
            "router://00000000-0000-4000-8000-000000000001/push/{}",
            public.push_id.as_str()
        )
    );
    assert_eq!(
        serde_json::to_value(public.receipt).expect("public receipt"),
        serde_json::to_value(witness.persisted).expect("stored receipt")
    );
}

fn assert_recorded_loaded_receipt(witness: &RecordedReceiptWitness, mode: MessageDelivery) {
    let RecordedReceiptWitness {
        observation,
        stored,
        persisted,
        native,
    } = witness;
    assert_eq!(
        stored.delivery_state,
        collaboration_protocol::PushDeliveryState::Delivered
    );
    assert_eq!(stored.target, observation.target);
    assert_eq!(stored.mode, Some(mode));
    assert_eq!(stored.guard, Some(observation.generation.clone()));
    assert_eq!(stored.body.as_deref(), Some("A finding"));
    assert!(stored.settled_at.is_some());
    assert_eq!(native.target, observation.target);
    assert_eq!(native.generation, observation.generation);
    assert_eq!(
        native.input_kind,
        collaboration_protocol::MessageInputKind::Agent
    );
    assert_eq!(
        native.representation,
        collaboration_protocol::MessageRepresentation::DeclaredAgentText
    );
    assert_eq!(
        native.resume_effect,
        collaboration_protocol::AcceptedResumeEffect::Accepted
    );
    assert_eq!(
        String::from(native.client_user_message_id.clone()),
        stored.push_id.as_str()
    );
    assert!(
        matches!(
            (mode, &persisted.outcome, &native.acceptance),
            (
                MessageDelivery::Queue,
                collaboration_protocol::DeliveryOutcome::Queued,
                NativeSendAcceptance::QueueAccepted { .. }
            ) | (
                MessageDelivery::Steer,
                collaboration_protocol::DeliveryOutcome::Steered,
                NativeSendAcceptance::SteerAccepted { .. }
            )
        ),
        "mode, stored outcome and upstream acceptance must agree"
    );
    if let NativeSendAcceptance::QueueAccepted { submission_id } = &native.acceptance {
        assert_eq!(String::from(submission_id.clone()), "recorded-queue-1");
    }
    if let NativeSendAcceptance::SteerAccepted {
        turn_id,
        submission_id,
    } = &native.acceptance
    {
        assert_eq!(String::from(turn_id.clone()), "recorded-turn-1");
        assert!(submission_id.is_none());
    }
    assert_eq!(
        observation
            .requests
            .get(1)
            .and_then(|request| request.get("params")),
        Some(&json!({"threadId":"target","excludeTurns":true}))
    );
    let expected_method = if mode == MessageDelivery::Queue {
        "thread/queue/add"
    } else {
        "turn/steer"
    };
    let input = observation.requests.last();
    assert_eq!(
        input.and_then(|request| request.get("method")),
        Some(&json!(expected_method))
    );
    assert_eq!(
        input.and_then(|request| request.pointer("/params/threadId")),
        Some(&json!("target"))
    );
    assert_eq!(
        input.and_then(|request| request.pointer("/params/clientUserMessageId")),
        Some(&json!(stored.push_id.as_str()))
    );
    assert_eq!(
        input.and_then(|request| request.pointer("/params/input/0/type")),
        Some(&json!("text"))
    );
    let text = input
        .and_then(|request| request.pointer("/params/input/0/text"))
        .and_then(serde_json::Value::as_str);
    assert!(
        text.is_some_and(|text| text.contains("\"A finding\"")),
        "native text must contain the exact body"
    );
    assert!(
        text.is_some_and(|text| text.ends_with(&format!(
            "router://00000000-0000-4000-8000-000000000001/push/{}",
            stored.push_id.as_str()
        ))),
        "native text must retain the complete exact link"
    );
    assert_eq!(
        observation.requests.len(),
        if mode == MessageDelivery::Queue { 3 } else { 4 }
    );
    if mode == MessageDelivery::Steer {
        assert_eq!(
            input.and_then(|request| request.pointer("/params/expectedTurnId")),
            Some(&json!("recorded-turn-1"))
        );
        assert_eq!(
            observation
                .requests
                .get(2)
                .and_then(|request| request.get("params")),
            Some(
                &json!({"threadId":"target","limit":1,"sortDirection":"desc","itemsView":"notLoaded"})
            )
        );
    }
}
