#![allow(clippy::expect_used)]
//! A permanent provider rejection must finish one wake delivery without retry eligibility.

use super::WakeDeliverySender;
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    AutomationConfigurationHandle, DeliveryFuture, DeliveryRequest, SessionMessageDelivery,
};
use agent_automation::{ExpiryRule, TimingRule};
use automation_storage::{AutomationStore, WakeCreate, WakeEvaluation};
use collaboration_protocol::{
    CodexGeneration, DeliveryNextAction, DeliveryOutcome, DeliveryReceipt, DeliveryRejection,
    DeliveryRejectionReason, OperationId, SavedMessage, SessionReachability, SessionRef,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

struct ProviderSessionNotFound;

impl SessionMessageDelivery for ProviderSessionNotFound {
    fn deliver<'a>(
        &'a self,
        _request: DeliveryRequest,
        _evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async {
            Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::Rejected(DeliveryRejection {
                    reason: DeliveryRejectionReason::ProviderSessionNotFound,
                    next_action: DeliveryNextAction::CorrectRequest,
                    client_code: Some(-32002),
                    detail: Some("this session never started a turn and did not survive the provider restart; create a new conversation".to_owned()),
                }),
                reachability: Some(SessionReachability::ProviderAcp),
                client: None,
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async { Ok(AttemptReconciliation::KnownNotSubmitted) })
    }
}

#[tokio::test]
async fn provider_session_not_found_finishes_wake_after_one_attempt() {
    let root = tempfile::tempdir().expect("automation root");
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.path().join("automation.sqlite"))
            .await
            .expect("automation store"),
    ));
    let now_ms = chrono::Utc::now().timestamp_millis() - 10_000;
    let message: SavedMessage = serde_json::from_value(json!({
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"empty-session"},
        "content":{"kind":"humanUser","text":"wake fixture"},
        "delivery":"auto"
    }))
    .expect("saved message without an explicit generation guard");
    let wake = store
        .lock()
        .await
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message,
            timing: TimingRule::After { seconds: 1 },
            expiry: ExpiryRule::None,
            now_ms,
        })
        .await
        .expect("wake creation");
    let wakeup_id = wake.definition.wakeup_id;
    let delivery_id = {
        let mut store = store.lock().await;
        let due_ids = store
            .due_wakeup_ids(now_ms + 5_000, 4)
            .await
            .expect("due wake inventory");
        assert_eq!(due_ids.as_slice(), std::slice::from_ref(&wakeup_id));
        match store
            .evaluate_wakeup::<SavedMessage>(&wakeup_id, now_ms + 5_000)
            .await
            .expect("wake evaluation")
        {
            WakeEvaluation::Fired { delivery_id, .. } => delivery_id,
            _ => panic!("eligible wake did not fire"),
        }
    };
    let sender = WakeDeliverySender {
        delivery: Arc::new(ProviderSessionNotFound),
        configuration: AutomationConfigurationHandle::default(),
    };

    sender
        .dispatch(Arc::clone(&store), delivery_id.clone())
        .await
        .expect("wake dispatch");

    let record = store
        .lock()
        .await
        .read_delivery::<SessionRef, CodexGeneration, serde_json::Value>(&delivery_id)
        .await
        .expect("delivery record");
    assert_eq!(
        serde_json::to_value(record.status).expect("status"),
        json!("failed")
    );
    assert_eq!(record.attempt.as_ref().expect("attempt").attempt_number, 1);
    assert_eq!(
        record.receipt.as_ref().expect("receipt")["outcome"]["reason"],
        "providerSessionNotFound"
    );
    assert_eq!(
        record.receipt.as_ref().expect("receipt")["outcome"]["clientCode"],
        -32002
    );
    assert!(
        store
            .lock()
            .await
            .eligible_delivery_ids(chrono::Utc::now().timestamp_millis() + 60_000, 4)
            .await
            .expect("future eligible deliveries")
            .is_empty()
    );
}
