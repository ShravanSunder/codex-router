//! Host policy adapter for the ACP client's provider interactions.

use std::sync::{Arc, Weak};
use std::time::Duration;

use acp_client_runtime::{
    ApprovalPortOutcome, InteractionFuture, InteractionPort, RefusedApprovalOffer, SessionEventSink,
};
use collaboration_protocol::OperationId;
use collaboration_service::{
    RefusedApprovalOption, RefusedTypedApproval, ServiceInteractionBroker,
};
use message_board::{Identity, SessionRef as BoardSessionRef};
use session_event_model::{
    ApprovalRequest, PendingInteraction, QuestionRequest, QuestionResponse, SessionEvent,
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::ExternalProviderApprovalContext;

const APPROVAL_DECISION_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
pub(crate) struct HostInteractionPort {
    broker: Arc<RwLock<Option<Weak<ServiceInteractionBroker>>>>,
    event_sink: Arc<RwLock<Option<Arc<dyn SessionEventSink>>>>,
}

impl HostInteractionPort {
    pub(crate) async fn install_broker(&self, broker: Arc<ServiceInteractionBroker>) {
        *self.broker.write().await = Some(Arc::downgrade(&broker));
    }

    async fn broker(&self) -> Option<Arc<ServiceInteractionBroker>> {
        self.broker.read().await.as_ref().and_then(Weak::upgrade)
    }

    pub(crate) async fn install_event_sink(&self, event_sink: Arc<dyn SessionEventSink>) {
        *self.event_sink.write().await = Some(event_sink);
    }

    async fn publish(&self, session_id: &str, event: SessionEvent) -> bool {
        let Some(event_sink) = self.event_sink.read().await.clone() else {
            return true;
        };
        if event_sink.publish(session_id, event).is_err() {
            tracing::error!(
                session_id,
                "provider interaction event could not be published"
            );
            return false;
        }
        true
    }
}

impl InteractionPort for HostInteractionPort {
    type Context = ExternalProviderApprovalContext;
    type OperationId = OperationId;

    fn operation_id(context: &Self::Context) -> Self::OperationId {
        context.operation_id.clone()
    }

    fn binding_retirement(context: &Self::Context) -> CancellationToken {
        context.binding_retirement.clone()
    }

    fn request_approval(
        &self,
        context: Self::Context,
        request: ApprovalRequest,
        turn_cancellation: CancellationToken,
        agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, ApprovalPortOutcome> {
        Box::pin(async move {
            let Some(broker) = self.broker().await else {
                return ApprovalPortOutcome::Unavailable;
            };
            let Some((requester, approver)) = typed_participants(&context) else {
                return ApprovalPortOutcome::Unavailable;
            };
            let request_id = request.request_id.clone();
            let session_id = String::from(context.target.session_id.clone());
            let pending_event = SessionEvent::InteractionRequested {
                interaction: PendingInteraction::Approval {
                    approver: approver.clone(),
                    request: Box::new(request.clone()),
                },
            };
            let receiver = match broker
                .request_typed_approval(
                    requester.clone(),
                    approver.clone(),
                    request.clone(),
                    turn_cancellation.clone(),
                    context.binding_retirement.clone(),
                    Some(collaboration_service::TypedApprovalLegacyContext {
                        operation_id: context.operation_id.clone(),
                        target: context.target.clone(),
                        generation: context.binding_generation.clone(),
                        requested_by: context.requester.clone(),
                    }),
                )
                .await
            {
                Ok(receiver) => receiver,
                Err(collaboration_service::InteractionHistoryError::NotPending) => {
                    return ApprovalPortOutcome::Cancelled;
                }
                Err(collaboration_service::InteractionHistoryError::SelfApprover) => {
                    let refusal = RefusedTypedApproval {
                        request_id: request.request_id,
                        title: request.title,
                        description: request.description,
                        subject: request.subject,
                        options: request
                            .options
                            .iter()
                            .map(|option| RefusedApprovalOption {
                                option_id: option.option_id.as_str().to_owned(),
                                label: option.label.clone(),
                                provider_kind: "selfApprover".to_owned(),
                            })
                            .collect(),
                        reason: "set a different approver".to_owned(),
                    };
                    if let Err(error) = broker
                        .record_typed_refusal(requester, approver, refusal)
                        .await
                    {
                        tracing::error!(%error, "self-approver refusal could not be recorded");
                    }
                    return ApprovalPortOutcome::Unavailable;
                }
                Err(error) => {
                    tracing::error!(%error, "provider approval could not be admitted");
                    return ApprovalPortOutcome::Unavailable;
                }
            };
            if !self.publish(&session_id, pending_event).await {
                let _ = broker
                    .cancel_typed_approval(&request_id, "eventPublicationFailed")
                    .await;
                return ApprovalPortOutcome::Unavailable;
            }
            tokio::pin!(receiver);
            let (outcome, resolved_here) = tokio::select! {
                biased;
                () = turn_cancellation.cancelled() => {
                    let settled = broker.cancel_typed_approval(&request_id, "turnCancelled").await.is_ok();
                    (approval_receiver_outcome(receiver.await), settled)
                }
                () = context.binding_retirement.cancelled() => {
                    let settled = broker.cancel_typed_approval(&request_id, "providerRetired").await.is_ok();
                    (approval_receiver_outcome(receiver.await), settled)
                }
                () = agent_cancellation.cancelled() => {
                    let settled = broker.cancel_typed_approval(&request_id, "agentCancelled").await.is_ok();
                    (approval_receiver_outcome(receiver.await), settled)
                }
                () = tokio::time::sleep(APPROVAL_DECISION_TIMEOUT) => {
                    let settled = broker.cancel_typed_approval(&request_id, "timedOut").await.is_ok();
                    (approval_receiver_outcome(receiver.await), settled)
                }
                selected = &mut receiver => {
                    let resolved_here = selected.is_ok()
                        || (!turn_cancellation.is_cancelled()
                            && !context.binding_retirement.is_cancelled()
                            && !agent_cancellation.is_cancelled());
                    (approval_receiver_outcome(selected), resolved_here)
                },
            };
            if resolved_here {
                let _ = self
                    .publish(
                        &session_id,
                        SessionEvent::InteractionResolved { request_id },
                    )
                    .await;
            }
            outcome
        })
    }

    fn request_question(
        &self,
        context: Self::Context,
        request: QuestionRequest,
        turn_cancellation: CancellationToken,
        agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, QuestionResponse> {
        Box::pin(async move {
            let Some(broker) = self.broker().await else {
                return QuestionResponse::Cancelled;
            };
            let Some((requester, approver)) = typed_participants(&context) else {
                return QuestionResponse::Cancelled;
            };
            if turn_cancellation.is_cancelled()
                || agent_cancellation.is_cancelled()
                || context.binding_retirement.is_cancelled()
            {
                let reason = if turn_cancellation.is_cancelled() {
                    "turnCancelled"
                } else if context.binding_retirement.is_cancelled() {
                    "providerRetired"
                } else {
                    "agentCancelled"
                };
                if let Err(error) = broker
                    .record_cancelled_question(requester, approver, request, reason)
                    .await
                {
                    tracing::error!(%error, "cancelled provider question could not be recorded");
                }
                return QuestionResponse::Cancelled;
            }
            let request_id = request.request_id.clone();
            let session_id = String::from(context.target.session_id.clone());
            let pending_event = SessionEvent::InteractionRequested {
                interaction: PendingInteraction::Question {
                    approver: approver.clone(),
                    request: Box::new(request.clone()),
                },
            };
            let receiver = match broker
                .request_question(
                    requester,
                    approver,
                    request,
                    Some(context.binding_retirement.clone()),
                )
                .await
            {
                Ok(receiver) => receiver,
                Err(error) => {
                    tracing::error!(%error, "provider question could not be admitted");
                    return QuestionResponse::Cancelled;
                }
            };
            if !self.publish(&session_id, pending_event).await {
                let _ = broker
                    .cancel_question(&request_id, "eventPublicationFailed")
                    .await;
                return QuestionResponse::Cancelled;
            }
            tokio::pin!(receiver);
            let (response, resolved_here) = tokio::select! {
                biased;
                () = turn_cancellation.cancelled() => {
                    let settled = broker.cancel_question(&request_id, "turnCancelled").await.is_ok();
                    (receiver.await.unwrap_or(QuestionResponse::Cancelled), settled)
                }
                () = context.binding_retirement.cancelled() => {
                    let settled = broker.cancel_question(&request_id, "providerRetired").await.is_ok();
                    (receiver.await.unwrap_or(QuestionResponse::Cancelled), settled)
                }
                () = agent_cancellation.cancelled() => {
                    let settled = broker.cancel_question(&request_id, "agentCancelled").await.is_ok();
                    (receiver.await.unwrap_or(QuestionResponse::Cancelled), settled)
                }
                settled = &mut receiver => {
                    let response = settled.unwrap_or(QuestionResponse::Cancelled);
                    let resolved_here = !turn_cancellation.is_cancelled()
                        && !context.binding_retirement.is_cancelled()
                        && !agent_cancellation.is_cancelled();
                    (response, resolved_here)
                },
            };
            if resolved_here {
                let _ = self
                    .publish(
                        &session_id,
                        SessionEvent::InteractionResolved { request_id },
                    )
                    .await;
            }
            response
        })
    }

    fn record_refusal(
        &self,
        context: Self::Context,
        refusal: RefusedApprovalOffer,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async move {
            let Some(broker) = self.broker().await else {
                tracing::error!(
                    "provider approval refusal could not be recorded: broker unavailable"
                );
                return;
            };
            let Some((requester, approver)) = typed_participants(&context) else {
                tracing::error!(
                    "provider approval refusal could not be recorded: invalid Session identity"
                );
                return;
            };
            let record = RefusedTypedApproval {
                request_id: refusal.request_id,
                title: refusal.title,
                description: refusal.description,
                subject: refusal.subject,
                options: refusal
                    .options
                    .into_iter()
                    .map(|option| RefusedApprovalOption {
                        option_id: option.option_id,
                        label: option.label,
                        provider_kind: option.provider_kind,
                    })
                    .collect(),
                reason: refusal.reason.to_owned(),
            };
            if let Err(error) = broker
                .record_typed_refusal(requester, approver, record)
                .await
            {
                tracing::error!(%error, "provider approval refusal could not be recorded");
            }
        })
    }

    fn cancel_all(
        &self,
        context: Self::Context,
        reason: &'static str,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async move {
            let Some(broker) = self.broker().await else {
                return;
            };
            let Some((requester, _)) = typed_participants(&context) else {
                return;
            };
            match broker.cancel_typed_approvals(&requester, reason).await {
                Ok(request_ids) => {
                    let session_id = String::from(context.target.session_id.clone());
                    for request_id in request_ids {
                        let _ = self
                            .publish(
                                &session_id,
                                SessionEvent::InteractionResolved { request_id },
                            )
                            .await;
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "failed to cancel provider approvals for Session")
                }
            }
            match broker.cancel_questions(&requester, reason).await {
                Ok(request_ids) => {
                    let session_id = String::from(context.target.session_id.clone());
                    for request_id in request_ids {
                        let _ = self
                            .publish(
                                &session_id,
                                SessionEvent::InteractionResolved { request_id },
                            )
                            .await;
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "failed to cancel provider questions for Session")
                }
            }
        })
    }

    fn cancel_retired(&self) -> InteractionFuture<'_, ()> {
        Box::pin(async move {
            if let Some(broker) = self.broker().await {
                match broker.cancel_retired_typed_approvals().await {
                    Ok(retired) => {
                        for (requester, request_id) in retired {
                            let _ = self
                                .publish(
                                    requester.session_id.as_str(),
                                    SessionEvent::InteractionResolved { request_id },
                                )
                                .await;
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "failed to cancel retired provider approvals")
                    }
                }
            }
        })
    }
}

fn typed_participants(
    context: &ExternalProviderApprovalContext,
) -> Option<(BoardSessionRef, Identity)> {
    let requester = board_session_ref(&context.target)?;
    let approver = context.approver.to_board_identity().ok()?;
    Some((requester, approver))
}

fn board_session_ref(value: &collaboration_protocol::SessionRef) -> Option<BoardSessionRef> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn approval_receiver_outcome(
    selected: Result<
        collaboration_service::TypedApprovalResolution,
        tokio::sync::oneshot::error::RecvError,
    >,
) -> ApprovalPortOutcome {
    match selected {
        Ok(collaboration_service::TypedApprovalResolution::Selected(selected)) => {
            ApprovalPortOutcome::Selected {
                option_id: selected.option_id.as_str().to_owned(),
                note: selected.note,
            }
        }
        Ok(collaboration_service::TypedApprovalResolution::Cancelled) => {
            ApprovalPortOutcome::Cancelled
        }
        Err(_) => ApprovalPortOutcome::Cancelled,
    }
}

/// Test-only stand-in for direct runtime fixtures without a Router Host.
pub(crate) struct NoopSessionEventSink;

impl SessionEventSink for NoopSessionEventSink {
    fn begin_history_replay(
        &self,
        _session_id: &str,
    ) -> acp_client_runtime::HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    fn publish(
        &self,
        _session_id: &str,
        _event: session_event_model::SessionEvent,
    ) -> Result<(), acp_client_runtime::EventSinkClosed> {
        Ok(())
    }
}

#[cfg(test)]
mod provider_actor_tests {
    use super::*;

    struct StalledNoticeDelivery {
        entered: Arc<tokio::sync::Notify>,
        dropped: Arc<tokio::sync::Notify>,
    }

    struct NoticeInFlight(Arc<tokio::sync::Notify>);

    impl Drop for NoticeInFlight {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }

    impl collaboration_service::SessionMessageDelivery for StalledNoticeDelivery {
        fn deliver<'a>(
            &'a self,
            _: collaboration_service::layer_zero::DeliveryRequest,
            _: &'a dyn collaboration_service::AttemptEvidenceSink,
        ) -> collaboration_service::DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt>
        {
            let entered = Arc::clone(&self.entered);
            let dropped = Arc::clone(&self.dropped);
            Box::pin(async move {
                let _in_flight = NoticeInFlight(dropped);
                entered.notify_one();
                std::future::pending().await
            })
        }

        fn reconcile_attempt(
            &self,
            _: collaboration_service::AttemptReconciliationContext,
        ) -> collaboration_service::DeliveryFuture<'_, collaboration_service::AttemptReconciliation>
        {
            Box::pin(async { Ok(collaboration_service::AttemptReconciliation::StillUnknown) })
        }
    }

    #[derive(Default)]
    struct RecordedInteractionSink(std::sync::Mutex<Vec<SessionEvent>>);

    impl SessionEventSink for RecordedInteractionSink {
        fn begin_history_replay(&self, _: &str) -> acp_client_runtime::HistoryReplayFuture<'_> {
            Box::pin(async { Ok(()) })
        }

        fn publish(
            &self,
            _: &str,
            event: SessionEvent,
        ) -> Result<(), acp_client_runtime::EventSinkClosed> {
            self.0.lock().expect("event lock").push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn stalled_session_notices_do_not_delay_turn_cancellation_or_resolution() {
        use acp_client_runtime::InteractionPort as _;
        let directory = tempfile::tempdir().expect("broker directory");
        let service_id: collaboration_protocol::UuidIdentity =
            "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89"
                .to_owned()
                .try_into()
                .expect("service ID");
        let endpoint = collaboration_protocol::EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: "cursor-local".to_owned().try_into().expect("endpoint ID"),
        };
        let broker = ServiceInteractionBroker::load(
            service_id.clone(),
            collaboration_service::NativeControlBackend {
                endpoint: endpoint.clone(),
                gate: collaboration_service::NativeGenerationGate::default(),
                codex_home: directory.path().to_owned(),
            },
            directory.path().join("approval-routes.json"),
        )
        .await
        .expect("broker");
        let push_store = Arc::new(tokio::sync::Mutex::new(
            automation_storage::AutomationStore::open(
                &directory.path().join("approval-pushes.sqlite"),
            )
            .await
            .expect("approval push store"),
        ));
        let machine_identity =
            collaboration_service::MachineIdentity::new(service_id.clone(), Some("fixture-host"))
                .expect("machine identity");
        let service_id_text = String::from(service_id.clone());
        let broker_context = collaboration_service::ServiceIdentity::new(
            &service_id_text,
            &service_id_text,
            &format!("sha256:{}", "a".repeat(64)),
        )
        .expect("service identity")
        .with_machine_identity(machine_identity)
        .expect("machine identity belongs to service")
        .with_automation_store(push_store)
        .with_approval_broker(Arc::clone(&broker));
        drop(broker_context);
        let notice_entered = Arc::new(tokio::sync::Notify::new());
        let notice_dropped = Arc::new(tokio::sync::Notify::new());
        broker
            .install_session_delivery(Arc::new(StalledNoticeDelivery {
                entered: Arc::clone(&notice_entered),
                dropped: Arc::clone(&notice_dropped),
            }))
            .expect("notice route");
        let sink = Arc::new(RecordedInteractionSink::default());
        let port = Arc::new(HostInteractionPort::default());
        port.install_broker(Arc::clone(&broker)).await;
        port.install_event_sink(Arc::clone(&sink) as Arc<dyn SessionEventSink>)
            .await;
        let target: collaboration_protocol::SessionRef =
            serde_json::from_value(serde_json::json!({
                "endpoint":endpoint,"sessionId":"provider-session"
            }))
            .expect("target");
        let approver: collaboration_protocol::SessionRef =
            serde_json::from_value(serde_json::json!({
                "endpoint":endpoint,"sessionId":"approver-session"
            }))
            .expect("approver");
        let context = || ExternalProviderApprovalContext {
            requester: target.clone().into(),
            approver: approver.clone().into(),
            target: target.clone(),
            operation_id: OperationId::generate(),
            binding_generation: serde_json::from_value(serde_json::json!({
                "serviceEpoch":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","generation":1
            }))
            .expect("generation"),
            binding_retirement: CancellationToken::new(),
        };
        let approval = serde_json::from_value(serde_json::json!({
            "requestId":"stalled-approval", "title":"Run command", "options":[
                {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}}
            ]
        })).expect("approval");
        let approval_cancel = CancellationToken::new();
        let approval_task = tokio::spawn({
            let port = Arc::clone(&port);
            let cancellation = approval_cancel.clone();
            let context = context();
            async move {
                port.request_approval(context, approval, cancellation, CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), notice_entered.notified())
            .await
            .expect("approval notice entered");
        approval_cancel.cancel();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_millis(500), approval_task)
                .await
                .expect("approval cancellation bounded")
                .expect("approval task"),
            ApprovalPortOutcome::Cancelled
        ));
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            notice_dropped.notified(),
        )
        .await
        .expect("settled approval aborted its notice task");
        let question = serde_json::from_value(serde_json::json!({
            "requestId":"stalled-question","prompt":"Proceed?","fields":[
                {"kind":"boolean","fieldId":"yes","label":"Yes","description":null,"required":true}
            ]
        }))
        .expect("question");
        let question_cancel = CancellationToken::new();
        let question_task = tokio::spawn({
            let port = Arc::clone(&port);
            let cancellation = question_cancel.clone();
            let context = context();
            async move {
                port.request_question(context, question, cancellation, CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), notice_entered.notified())
            .await
            .expect("question notice entered");
        question_cancel.cancel();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_millis(500), question_task)
                .await
                .expect("question cancellation bounded")
                .expect("question task"),
            QuestionResponse::Cancelled
        );
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            notice_dropped.notified(),
        )
        .await
        .expect("settled question aborted its notice task");
        let events = sink.0.lock().expect("event lock");
        assert_eq!(events.len(), 4);
        assert!(matches!(
            events.first(),
            Some(SessionEvent::InteractionRequested { .. })
        ));
        assert!(
            matches!(events.get(1), Some(SessionEvent::InteractionResolved { request_id }) if request_id == "stalled-approval")
        );
        assert!(matches!(
            events.get(2),
            Some(SessionEvent::InteractionRequested { .. })
        ));
        assert!(
            matches!(events.get(3), Some(SessionEvent::InteractionResolved { request_id }) if request_id == "stalled-question")
        );
    }

    #[test]
    fn human_approver_reaches_typed_broker_without_a_synthetic_session() {
        let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","endpointId":"cursor-local"},
            "sessionId":"provider-session"
        })).expect("target");
        let context = ExternalProviderApprovalContext {
            requester: target.clone().into(),
            approver: serde_json::from_value(serde_json::json!({"humanId":"owner"}))
                .expect("human"),
            target,
            operation_id: collaboration_protocol::OperationId::generate(),
            binding_generation: serde_json::from_value(serde_json::json!({
                "serviceEpoch":"1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80","generation":1
            }))
            .expect("generation"),
            binding_retirement: CancellationToken::new(),
        };
        let (_, approver) = typed_participants(&context).expect("broker participants");
        assert!(matches!(approver, Identity::Human { human_id } if human_id.as_str() == "owner"));
    }
}
