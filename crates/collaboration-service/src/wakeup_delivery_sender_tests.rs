#![allow(clippy::expect_used)]
//! A permanent provider rejection must finish one wake delivery without retry eligibility.

use super::WakeDeliverySender;
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    AutomationConfigurationHandle, DeliveryContractError, DeliveryFuture, DeliveryRequest,
    ServiceIdentity, SessionMessageDelivery, session_message_dispatch,
    session_message_reply_dispatch,
};
use agent_automation::{
    ClaudeCodePeerEffectEvidence, ExpiryRule, OperationId, PeerProcessId, PeerSessionReference,
    PeerWriteEffect, RouteEffectEvidence, TimingRule,
};
use automation_storage::{AutomationStore, WakeCreate, WakeEvaluation};
use collaboration_protocol::{
    CodexGeneration, DeliveryClientReceipt, DeliveryNextAction, DeliveryOutcome, DeliveryReceipt,
    DeliveryRejection, DeliveryRejectionReason, MessageContent, MessageDelivery, MessageText,
    SavedMessage, SessionMessageSendParams, SessionReachability, SessionRef,
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

#[derive(Default)]
struct AcceptedMessageDelivery {
    requests: std::sync::Mutex<Vec<DeliveryRequest>>,
}

impl SessionMessageDelivery for AcceptedMessageDelivery {
    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            let peer_session_id: PeerSessionReference =
                String::from(request.target.session_id.clone())
                    .try_into()
                    .map_err(|_| DeliveryContractError::InvalidEvidence)?;
            let process_id: PeerProcessId = 42_u32
                .try_into()
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
            evidence
                .record(RouteEffectEvidence::ClaudeCodePeer(
                    ClaudeCodePeerEffectEvidence {
                        session_id: peer_session_id.clone(),
                        process_id,
                        write: PeerWriteEffect::Dispatching,
                    },
                ))
                .await?;
            self.requests
                .lock()
                .map_err(|_| DeliveryContractError::ClientOperation)?
                .push(request);
            evidence
                .record(RouteEffectEvidence::ClaudeCodePeer(
                    ClaudeCodePeerEffectEvidence {
                        session_id: peer_session_id,
                        process_id,
                        write: PeerWriteEffect::Written,
                    },
                ))
                .await?;
            Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::PeerMessageWritten,
                reachability: Some(SessionReachability::ClaudeCodePeer),
                client: Some(DeliveryClientReceipt::ClaudeCodePeer),
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async { Ok(AttemptReconciliation::StillUnknown) })
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
        display_names: crate::SessionDisplayNameCache::default(),
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

#[tokio::test]
async fn self_wake_does_not_replace_the_latest_agent_sender_for_reply() {
    let root = tempfile::tempdir().expect("automation root");
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.path().join("automation.sqlite"))
            .await
            .expect("automation store"),
    ));
    let real_sender: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
        "sessionId":"real-agent-sender"
    }))
    .expect("real sender reference");
    let caller: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"reply-recipient"
    }))
    .expect("caller reference");
    let recorded_delivery = Arc::new(AcceptedMessageDelivery::default());
    let delivery: Arc<dyn SessionMessageDelivery> = Arc::clone(&recorded_delivery) as _;
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000001",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .expect("service identity")
    .with_automation_store(Arc::clone(&store))
    .with_session_delivery(Arc::clone(&delivery));

    let direct = session_message_dispatch::dispatch(
        json!("direct-send"),
        serde_json::to_value(SessionMessageSendParams {
            target: caller.clone(),
            message: MessageContent::Agent {
                sender: real_sender.clone(),
                text: MessageText::try_from("real Agent message".to_owned()).expect("message text"),
            },
            mode: MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .expect("direct message parameters"),
        &identity,
    )
    .await;
    assert_eq!(direct["result"]["outcome"]["kind"], "peerMessageWritten");

    let now_ms = chrono::Utc::now().timestamp_millis() - 10_000;
    let self_wake: SavedMessage = serde_json::from_value(json!({
        "target":caller,
        "content":{"kind":"agent","sender":caller,"text":"self wake"},
        "delivery":"auto",
        "generationGuard":null
    }))
    .expect("self wake message");
    let wake = store
        .lock()
        .await
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message: self_wake,
            timing: TimingRule::After { seconds: 1 },
            expiry: ExpiryRule::None,
            now_ms,
        })
        .await
        .expect("create self wake");
    let delivery_id = match store
        .lock()
        .await
        .evaluate_wakeup::<SavedMessage>(
            &wake.definition.wakeup_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .expect("fire self wake")
    {
        WakeEvaluation::Fired { delivery_id, .. } => delivery_id,
        _ => panic!("self wake should fire"),
    };
    WakeDeliverySender {
        delivery: identity
            .session_message_delivery()
            .expect("message delivery route"),
        configuration: identity.configuration.clone(),
        display_names: identity.session_display_name_cache(),
    }
    .dispatch(Arc::clone(&store), delivery_id)
    .await
    .expect("self wake delivery");

    let reply = session_message_reply_dispatch::dispatch(
        json!("reply"),
        serde_json::to_value(collaboration_protocol::SessionMessageReplyParams {
            caller: caller.clone(),
            expect_sender: None,
            text: MessageText::try_from("reply to real sender".to_owned()).expect("reply text"),
        })
        .expect("reply parameters"),
        &identity,
    )
    .await;
    assert_eq!(
        reply["result"]["receipt"]["outcome"]["kind"],
        "peerMessageWritten"
    );
    let requests = recorded_delivery
        .requests
        .lock()
        .expect("recorded deliveries");
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].target, real_sender);
}
