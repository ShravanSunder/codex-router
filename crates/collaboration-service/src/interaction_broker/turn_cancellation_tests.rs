//! External approval admission and decision races with a Router turn cancel.

use super::*;
use collaboration_protocol::{CodexGeneration, OperationId};
use tokio::sync::Notify;
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
    broker: &ServiceApprovalBroker,
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
    *broker.typed_after_record.lock().await = Some(ExternalAdmissionPause {
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
        )
        .await
        .expect("first pending");
    let second = broker
        .request_typed_approval(
            other,
            approver,
            typed_request("second"),
            CancellationToken::new(),
        )
        .await
        .expect("second pending");
    assert_eq!(
        broker
            .cancel_typed_approvals(&requester, "turnCancelled")
            .await
            .expect("cancel first"),
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

fn external_request(
    broker: &ServiceApprovalBroker,
    generation: CodexGeneration,
    turn_cancellation: CancellationToken,
) -> ExternalApprovalRequest {
    let requester = super::tests::session(&broker.service_id, "requester");
    ExternalApprovalRequest {
        requester: requester.clone(),
        approver: requester,
        generation: generation.clone(),
        retirement: CancellationToken::new(),
        cancellation: CancellationToken::new(),
        turn_cancellation,
        operation_metadata: ExternalApprovalOperationMetadata {
            operation_id: OperationId::generate(),
            target: super::tests::session(&broker.service_id, "provider-session"),
            binding_generation: generation,
            method: "session/request_permission",
        },
        presentation: None,
        options: vec![ExternalApprovalOption {
            option_id: "allow-once".to_owned(),
            label: Some("Allow once".to_owned()),
            scope: ExternalApprovalOptionScope::AllowOnce,
        }],
    }
}

struct ObservedApprovalDelivery(Arc<Notify>);

impl crate::SessionMessageDelivery for ObservedApprovalDelivery {
    fn deliver<'a>(
        &'a self,
        _: crate::DeliveryRequest,
        _: &'a dyn crate::AttemptEvidenceSink,
    ) -> crate::DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt> {
        Box::pin(async move {
            self.0.notify_one();
            Ok(collaboration_protocol::DeliveryReceipt {
                outcome: DeliveryOutcome::Started,
                reachability: Some(collaboration_protocol::SessionReachability::CodexAppServer),
                client: None,
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _: crate::AttemptReconciliationContext,
    ) -> crate::DeliveryFuture<'_, crate::AttemptReconciliation> {
        Box::pin(async { Ok(crate::AttemptReconciliation::StillUnknown) })
    }
}

#[tokio::test]
async fn turn_cancelled_before_external_request_first_poll_records_one_terminal_row() {
    let (broker, generation, _directory) = super::tests::fixture_broker().await;
    let turn_cancellation = CancellationToken::new();
    let request = broker.request_external(external_request(
        &broker,
        generation,
        turn_cancellation.clone(),
    ));
    turn_cancellation.cancel();

    assert_eq!(
        request.await.expect("broker settles"),
        BrokeredApprovalOutcome::Cancelled
    );
    assert!(broker.pending.lock().await.is_empty());
    let history = broker.list(false).await.approvals;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state, ApprovalState::Cancelled);
    assert_eq!(history[0].reason.as_deref(), Some("turnCancelled"));
}

#[tokio::test]
async fn turn_cancelled_after_record_before_insert_leaves_no_pending_row() {
    let (broker, generation, _directory) = super::tests::fixture_broker().await;
    let turn_cancellation = CancellationToken::new();
    let (recorded_tx, recorded_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    *broker.external_after_record.lock().await = Some(ExternalAdmissionPause {
        recorded: recorded_tx,
        resume: resume_rx,
    });
    let request = external_request(&broker, generation, turn_cancellation.clone());
    let request_task = tokio::spawn({
        let broker = Arc::clone(&broker);
        async move { broker.request_external(request).await }
    });
    tokio::time::timeout(Duration::from_secs(5), recorded_rx)
        .await
        .expect("recorded boundary reached")
        .expect("boundary signal");
    assert_eq!(
        broker.list(false).await.approvals[0].state,
        ApprovalState::PendingClientDecision
    );
    turn_cancellation.cancel();
    resume_tx.send(()).expect("resume broker admission");

    assert_eq!(
        request_task
            .await
            .expect("broker task")
            .expect("settlement"),
        BrokeredApprovalOutcome::Cancelled
    );
    assert!(broker.pending.lock().await.is_empty());
    let history = broker.list(false).await.approvals;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state, ApprovalState::Cancelled);
    assert_eq!(history[0].reason.as_deref(), Some("turnCancelled"));
}

#[tokio::test]
async fn human_decision_after_turn_token_loses_to_cancelled_settlement() {
    let (broker, generation, _directory) = super::tests::fixture_broker().await;
    let notice = Arc::new(Notify::new());
    broker
        .install_session_delivery(Arc::new(ObservedApprovalDelivery(Arc::clone(&notice))))
        .expect("notice delivery");
    let turn_cancellation = CancellationToken::new();
    let request = external_request(&broker, generation, turn_cancellation.clone());
    let approver = request.approver.clone();
    let request_task = tokio::spawn({
        let broker = Arc::clone(&broker);
        async move { broker.request_external(request).await }
    });
    tokio::time::timeout(Duration::from_secs(5), notice.notified())
        .await
        .expect("request admitted and notice delivered");
    let request_id = broker.list(true).await.approvals[0].request_id.clone();
    turn_cancellation.cancel();
    let decision = broker
        .decide(ApprovalDecideParams {
            request_id,
            decision: Some(ApprovalDecision::Allow),
            option_id: None,
            acknowledge_persistent: false,
            actor: board_identity(&approver).expect("approver identity"),
        })
        .await;

    assert!(matches!(
        decision,
        Err(ApprovalDecisionError::Code("approvalNotPending"))
    ));
    assert_eq!(
        request_task
            .await
            .expect("broker task")
            .expect("settlement"),
        BrokeredApprovalOutcome::Cancelled
    );
    assert!(broker.pending.lock().await.is_empty());
    let history = broker.list(false).await.approvals;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state, ApprovalState::Cancelled);
    assert_eq!(history[0].reason.as_deref(), Some("turnCancelled"));
}
