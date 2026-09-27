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
    let approver = message_board::Identity::Session {
        session: super::board_session_ref(&super::tests::session(&broker.service_id, "approver"))
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
        Some(InteractionHistoryState::Cancelled { reason }) if reason == "turnCancelled"
    ));
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
        Some(InteractionHistoryState::Cancelled { reason }) if reason == "turnCancelled"
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
        Some(InteractionHistoryState::Cancelled { reason }) if reason == "turnCancelled"
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
