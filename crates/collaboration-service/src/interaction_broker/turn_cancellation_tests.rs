//! Typed approval admission and decision races with a Router turn cancel.

use super::*;
use tokio_util::sync::CancellationToken;

fn typed_request(request_id: &str) -> session_event_model::ApprovalRequest {
    serde_json::from_value(serde_json::json!({
        "requestId": request_id,
        "title": "Run command",
        "options": [{"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}}]
    }))
    .expect("typed approval")
}

fn typed_participants(
    broker: &ServiceInteractionBroker,
) -> (message_board::SessionRef, message_board::Identity) {
    let requester = super::board_session_ref(&super::tests::session(
        &broker.service_id,
        "provider-session",
    ))
    .expect("requester");
    let approver = message_board::Identity::Human {
        human_id: "turn-cancel-approver"
            .to_owned()
            .try_into()
            .expect("approver"),
    };
    (requester, approver)
}

#[tokio::test]
async fn typed_approval_cancelled_before_admission_records_turn_cancelled() {
    let (broker, _, _) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let turn_cancellation = CancellationToken::new();
    turn_cancellation.cancel();
    let result = broker
        .request_typed_approval(
            requester,
            approver,
            typed_request("typed-before-admission"),
            turn_cancellation,
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(InteractionHistoryError::NotPending)));
    let record = broker
        .interaction_history
        .interaction("typed-before-admission")
        .await
        .expect("terminal history");
    assert!(matches!(
        record.approval_state(),
        Some(InteractionHistoryState::Cancelled { reason }) if reason.as_str() == "turnCancelled"
    ));
    let row = broker
        .list_detailed(false)
        .await
        .expect("detailed list")
        .approvals
        .into_iter()
        .find(|row| row.request_id == "typed-before-admission")
        .expect("cancelled approval listed");
    assert_eq!(row.reason.as_deref(), Some("turnCancelled"));
}

#[tokio::test]
async fn typed_decision_after_turn_cancel_is_not_pending() {
    let (broker, _, _) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let turn_cancellation = CancellationToken::new();
    let receiver = broker
        .request_typed_approval(
            requester,
            approver.clone(),
            typed_request("typed-decision-race"),
            turn_cancellation.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("pending request");
    turn_cancellation.cancel();
    assert!(matches!(
        broker
            .decide(ApprovalDecideParams {
                request_id: "typed-decision-race".into(),
                decision: None,
                option_id: Some("allow-once".into()),
                acknowledge_persistent: false,
                note: None,
                actor: approver,
            })
            .await,
        Err(ApprovalDecisionError::Code("approvalNotPending"))
    ));
    assert!(receiver.await.is_err());
    let record = broker
        .interaction_history
        .interaction("typed-decision-race")
        .await
        .expect("terminal history");
    assert!(matches!(
        record.approval_state(),
        Some(InteractionHistoryState::Cancelled { reason }) if reason.as_str() == "turnCancelled"
    ));
}

#[tokio::test]
async fn typed_turn_cancelled_after_record_before_insert_leaves_no_pending_row() {
    let (broker, _, _) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let turn_cancellation = CancellationToken::new();
    let (recorded_tx, recorded_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    *broker.typed_after_record.lock().await = Some(TypedAdmissionPause {
        recorded: recorded_tx,
        resume: resume_rx,
    });
    let request_task = tokio::spawn({
        let broker = Arc::clone(&broker);
        let turn_cancellation = turn_cancellation.clone();
        async move {
            broker
                .request_typed_approval(
                    requester,
                    approver,
                    typed_request("typed-after-record"),
                    turn_cancellation,
                    CancellationToken::new(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), recorded_rx)
        .await
        .expect("recorded boundary reached")
        .expect("boundary signal");
    assert!(
        broker.typed_pending_approvals.try_lock().is_ok(),
        "history admission must not hold the pending map lock across file I/O"
    );
    turn_cancellation.cancel();
    resume_tx.send(()).expect("resume broker admission");
    assert!(matches!(
        request_task.await.expect("request task"),
        Err(InteractionHistoryError::NotPending)
    ));
    assert!(broker.typed_pending_approvals.lock().await.is_empty());
    let record = broker
        .interaction_history
        .interaction("typed-after-record")
        .await
        .expect("terminal history");
    assert!(matches!(
        record.approval_state(),
        Some(InteractionHistoryState::Cancelled { reason }) if reason.as_str() == "turnCancelled"
    ));
}

#[tokio::test]
async fn typed_session_cancellation_settles_only_that_sessions_requests() {
    let (broker, _, _) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let other = super::board_session_ref(&super::tests::session(&broker.service_id, "other"))
        .expect("other requester");
    let first = broker
        .request_typed_approval(
            requester.clone(),
            approver.clone(),
            typed_request("first"),
            CancellationToken::new(),
            CancellationToken::new(),
        )
        .await
        .expect("first pending");
    let second = broker
        .request_typed_approval(
            other,
            approver,
            typed_request("second"),
            CancellationToken::new(),
            CancellationToken::new(),
        )
        .await
        .expect("second pending");
    assert_eq!(
        broker
            .cancel_typed_approvals(&requester, "turnCancelled")
            .await
            .expect("cancel first")
            .len(),
        1
    );
    assert!(first.await.is_err());
    assert!(
        broker
            .typed_pending_approvals
            .lock()
            .await
            .contains_key("second")
    );
    drop(second);
}

fn typed_question(request_id: &str) -> session_event_model::QuestionRequest {
    serde_json::from_value(serde_json::json!({
        "requestId": request_id,
        "prompt": "Proceed?",
        "fields": [{"kind":"boolean","fieldId":"yes","label":"Yes","description":null,"required":true}]
    }))
    .expect("typed question")
}

#[tokio::test]
async fn dead_question_receiver_is_cancelled_in_history_before_answer() {
    let (broker, _, _) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let receiver = broker
        .request_question(
            requester,
            approver.clone(),
            typed_question("dead-question"),
            None,
        )
        .await
        .expect("question admitted");
    drop(receiver);
    assert!(matches!(
        broker
            .respond_question(
                "dead-question",
                &approver,
                QuestionResponse::Answered {
                    content: serde_json::from_value(serde_json::json!({"yes":true}))
                        .expect("answer")
                }
            )
            .await,
        Err(InteractionHistoryError::NotPending)
    ));
    let record = broker
        .interaction_history
        .interaction("dead-question")
        .await
        .expect("history");
    assert!(matches!(
        record,
        InteractionHistoryRecord::Question {
            state: QuestionHistoryState::Cancelled { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn receiver_closed_during_answer_never_persists_answered() {
    let (broker, _, _) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let receiver = broker
        .request_question(
            requester,
            approver.clone(),
            typed_question("closing-question"),
            None,
        )
        .await
        .expect("question admitted");
    let (recorded_tx, recorded_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    *broker.question_before_send.lock().await = Some(TypedAdmissionPause {
        recorded: recorded_tx,
        resume: resume_rx,
    });
    let answer_task = tokio::spawn({
        let broker = Arc::clone(&broker);
        async move {
            broker
                .respond_question(
                    "closing-question",
                    &approver,
                    QuestionResponse::Answered {
                        content: serde_json::from_value(serde_json::json!({"yes": true}))
                            .expect("answer"),
                    },
                )
                .await
        }
    });
    recorded_rx.await.expect("before-send boundary");
    let recorded = broker
        .interaction_history
        .interaction("closing-question")
        .await
        .expect("history");
    assert!(matches!(
        recorded,
        InteractionHistoryRecord::Question {
            state: QuestionHistoryState::Pending,
            ..
        }
    ));
    drop(receiver);
    resume_tx.send(()).expect("resume response");
    assert!(matches!(
        answer_task.await.expect("answer task"),
        Err(InteractionHistoryError::NotPending)
    ));
    let recorded = broker
        .interaction_history
        .interaction("closing-question")
        .await
        .expect("history");
    assert!(matches!(
        recorded,
        InteractionHistoryRecord::Question {
            state: QuestionHistoryState::Cancelled { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn question_answer_delivery_reports_later_history_write_failure() {
    let (broker, _, directory) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let receiver = broker
        .request_question(
            requester,
            approver.clone(),
            typed_question("write-failure-question"),
            None,
        )
        .await
        .expect("question admitted");
    tokio::fs::create_dir(directory.join("interaction-history.json.tmp"))
        .await
        .expect("block temporary history file");
    let answer = QuestionResponse::Answered {
        content: serde_json::from_value(serde_json::json!({"yes": true})).expect("answer"),
    };
    assert!(matches!(
        broker
            .respond_question("write-failure-question", &approver, answer.clone())
            .await,
        Err(InteractionHistoryError::Unavailable)
    ));
    assert_eq!(receiver.await.expect("response was delivered"), answer);
    assert!(
        !broker
            .pending_questions
            .lock()
            .await
            .contains_key("write-failure-question")
    );
    let record = broker
        .interaction_history
        .interaction("write-failure-question")
        .await
        .expect("history");
    assert!(matches!(
        record,
        InteractionHistoryRecord::Question {
            state: QuestionHistoryState::Pending,
            ..
        }
    ));
}

#[tokio::test]
async fn only_approver_can_explicitly_cancel_a_pending_approval() {
    let (broker, _, _) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let wrong_actor = message_board::Identity::Human {
        human_id: "other".to_owned().try_into().expect("other human"),
    };
    let receiver = broker
        .request_typed_approval(
            requester,
            approver.clone(),
            typed_request("approver-cancel-approval"),
            CancellationToken::new(),
            CancellationToken::new(),
        )
        .await
        .expect("approval pending");
    assert!(matches!(
        broker
            .decide_typed_interaction(
                "approver-cancel-approval",
                &wrong_actor,
                TypedInteractionDecision::Cancel
            )
            .await,
        Err(InteractionHistoryError::WrongActor)
    ));
    assert!(matches!(
        broker
            .interaction_history
            .interaction("approver-cancel-approval")
            .await,
        Some(InteractionHistoryRecord::Approval {
            state: InteractionHistoryState::Pending,
            ..
        })
    ));
    assert_eq!(
        broker
            .decide_typed_interaction(
                "approver-cancel-approval",
                &approver,
                TypedInteractionDecision::Cancel
            )
            .await
            .expect("approver cancelled"),
        TypedInteractionDecisionOutcome::ApprovalCancelled
    );
    assert_eq!(
        receiver.await.expect("agent cancellation"),
        TypedApprovalResolution::Cancelled
    );
    assert!(matches!(
        broker
            .interaction_history
            .interaction("approver-cancel-approval")
            .await,
        Some(InteractionHistoryRecord::Approval {
            state: InteractionHistoryState::Cancelled {
                reason: session_event_model::InteractionCancelReason::ApproverCancelled
            },
            ..
        })
    ));
    let record = broker
        .interaction_history
        .interaction("approver-cancel-approval")
        .await
        .expect("history");
    assert_eq!(
        serde_json::to_value(record).expect("wire state")["state"]["reason"],
        "approverCancelled"
    );
}

#[tokio::test]
async fn only_approver_can_explicitly_cancel_a_pending_question() {
    let (broker, _, _) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let wrong_actor = message_board::Identity::Human {
        human_id: "other".to_owned().try_into().expect("other human"),
    };
    let receiver = broker
        .request_question(
            requester,
            approver.clone(),
            typed_question("approver-cancel-question"),
            None,
        )
        .await
        .expect("question pending");
    assert!(matches!(
        broker
            .decide_typed_interaction(
                "approver-cancel-question",
                &wrong_actor,
                TypedInteractionDecision::Cancel
            )
            .await,
        Err(InteractionHistoryError::WrongActor)
    ));
    assert!(matches!(
        broker
            .interaction_history
            .interaction("approver-cancel-question")
            .await,
        Some(InteractionHistoryRecord::Question {
            state: QuestionHistoryState::Pending,
            ..
        })
    ));
    assert_eq!(
        broker
            .decide_typed_interaction(
                "approver-cancel-question",
                &approver,
                TypedInteractionDecision::Cancel
            )
            .await
            .expect("approver cancelled"),
        TypedInteractionDecisionOutcome::QuestionCancelled
    );
    assert_eq!(
        receiver.await.expect("agent cancellation"),
        QuestionResponse::Cancelled
    );
    assert!(
        matches!(broker.interaction_history.interaction("approver-cancel-question").await,
        Some(InteractionHistoryRecord::Question { state: QuestionHistoryState::Cancelled { reason }, .. }) if reason == session_event_model::InteractionCancelReason::ApproverCancelled)
    );
    let record = broker
        .interaction_history
        .interaction("approver-cancel-question")
        .await
        .expect("history");
    assert_eq!(
        serde_json::to_value(record).expect("wire state")["state"]["reason"],
        "approverCancelled"
    );
}

#[tokio::test]
async fn retired_requester_questions_are_cancelled_with_approvals() {
    let (broker, _, _) = super::tests::fixture_broker().await;
    let (requester, approver) = typed_participants(&broker);
    let retirement = CancellationToken::new();
    let approval = broker
        .request_typed_approval(
            requester.clone(),
            approver.clone(),
            typed_request("retired-approval"),
            CancellationToken::new(),
            retirement.clone(),
        )
        .await
        .expect("approval admitted");
    let question = broker
        .request_question(
            requester.clone(),
            approver,
            typed_question("retired-question"),
            Some(retirement.clone()),
        )
        .await
        .expect("question admitted");
    let question_only_requester =
        super::board_session_ref(&super::tests::session(&broker.service_id, "question-only"))
            .expect("question-only requester");
    let (_, question_only_approver) = typed_participants(&broker);
    let question_only = broker
        .request_question(
            question_only_requester,
            question_only_approver,
            typed_question("retired-question-only"),
            Some(retirement.clone()),
        )
        .await
        .expect("question-only admitted");
    retirement.cancel();
    let retired = broker
        .cancel_retired_typed_approvals()
        .await
        .expect("retired interactions");
    assert_eq!(retired.len(), 3);
    assert!(approval.await.is_err());
    assert_eq!(
        question.await.expect("question settled"),
        QuestionResponse::Cancelled
    );
    assert_eq!(
        question_only.await.expect("question-only settled"),
        QuestionResponse::Cancelled
    );
    let record = broker
        .interaction_history
        .interaction("retired-question")
        .await
        .expect("history");
    assert!(
        matches!(record, InteractionHistoryRecord::Question { state: QuestionHistoryState::Cancelled { reason }, .. } if reason.as_str() == "providerRetired")
    );
}
