//! Host policy adapter for the ACP client's provider interactions.

use std::sync::{Arc, Weak};

use acp_client_runtime::{
    ApprovalPortOutcome, InteractionFuture, InteractionPort, RefusedApprovalOffer, SessionEventSink,
};
use collaboration_protocol::OperationId;
use collaboration_service::{RefusedApprovalOption, RefusedTypedApproval, ServiceApprovalBroker};
use message_board::{Identity, SessionRef as BoardSessionRef};
use session_event_model::{ApprovalRequest, PendingInteraction, SessionEvent};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::ExternalProviderApprovalContext;

#[derive(Clone, Default)]
pub(crate) struct HostInteractionPort {
    broker: Arc<RwLock<Option<Weak<ServiceApprovalBroker>>>>,
    event_sink: Arc<RwLock<Option<Arc<dyn SessionEventSink>>>>,
}

impl HostInteractionPort {
    pub(crate) async fn install_broker(&self, broker: Arc<ServiceApprovalBroker>) {
        *self.broker.write().await = Some(Arc::downgrade(&broker));
    }

    async fn broker(&self) -> Option<Arc<ServiceApprovalBroker>> {
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
                    requester,
                    approver,
                    request,
                    turn_cancellation.clone(),
                    context.binding_retirement.clone(),
                )
                .await
            {
                Ok(receiver) => receiver,
                Err(collaboration_service::InteractionHistoryError::NotPending) => {
                    return ApprovalPortOutcome::Cancelled;
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
                selected = &mut receiver => {
                    let resolved_here = selected.is_ok();
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
            if let Err(error) = broker.cancel_questions(&requester, reason).await {
                tracing::error!(%error, "failed to cancel provider questions for Session");
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
    let approver = board_session_ref(&context.approver)?;
    Some((requester, Identity::Session { session: approver }))
}

fn board_session_ref(value: &collaboration_protocol::SessionRef) -> Option<BoardSessionRef> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn approval_receiver_outcome(
    selected: Result<session_event_model::OfferedOptionId, tokio::sync::oneshot::error::RecvError>,
) -> ApprovalPortOutcome {
    match selected {
        Ok(option_id) => ApprovalPortOutcome::Selected {
            option_id: option_id.as_str().to_owned(),
        },
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
    ) -> Result<(), acp_client_runtime::EventSinkOverflow> {
        Ok(())
    }
}
