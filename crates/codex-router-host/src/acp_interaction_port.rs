//! Host policy adapter for the ACP client's provider interactions.

use std::sync::{Arc, Weak};

use acp_client_runtime::{
    ApprovalPortOutcome, ExternalPermissionOptionMapping, InteractionFuture, InteractionPort,
    SessionEventSink,
};
use collaboration_protocol::{ApprovalArgument, ApprovalPresentation, OperationId};
use collaboration_service::{
    BrokeredApprovalOutcome, ExternalApprovalOperationMetadata, ExternalApprovalOption,
    ExternalApprovalOptionScope, ExternalApprovalRefusal, ExternalApprovalRequest,
    ServiceApprovalBroker,
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::ExternalProviderApprovalContext;

#[derive(Clone, Default)]
pub(crate) struct HostInteractionPort {
    broker: Arc<RwLock<Option<Weak<ServiceApprovalBroker>>>>,
}

impl HostInteractionPort {
    pub(crate) async fn install_broker(&self, broker: Arc<ServiceApprovalBroker>) {
        *self.broker.write().await = Some(Arc::downgrade(&broker));
    }

    async fn broker(&self) -> Option<Arc<ServiceApprovalBroker>> {
        self.broker.read().await.as_ref().and_then(Weak::upgrade)
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
        presentation: acp_client_runtime::ApprovalPresentation,
        mapping: ExternalPermissionOptionMapping,
        turn_cancellation: CancellationToken,
        agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, ApprovalPortOutcome> {
        Box::pin(async move {
            let Some(broker) = self.broker().await else {
                return ApprovalPortOutcome::Unavailable;
            };
            let operation_metadata = ExternalApprovalOperationMetadata {
                operation_id: context.operation_id,
                target: context.target,
                binding_generation: context.binding_generation.clone(),
                method: "session/request_permission",
            };
            let presentation = Some(broker_presentation(presentation));
            let (options, refusal_reason) = match mapping {
                ExternalPermissionOptionMapping::Mapped(options) => (broker_options(options), None),
                ExternalPermissionOptionMapping::Refused { reason, options } => {
                    (broker_options(options), Some(reason))
                }
            };
            if let Some(reason) = refusal_reason {
                if let Err(error) = broker
                    .record_external_refusal(ExternalApprovalRefusal {
                        requester: context.requester,
                        approver: context.approver,
                        generation: context.binding_generation,
                        operation_metadata,
                        offered_options: options,
                        presentation,
                        reason: reason.to_owned(),
                    })
                    .await
                {
                    tracing::error!(%error, "failed to record refused provider permission request");
                }
                return ApprovalPortOutcome::Cancelled;
            }
            match broker
                .request_external(ExternalApprovalRequest {
                    requester: context.requester,
                    approver: context.approver,
                    generation: context.binding_generation,
                    retirement: context.binding_retirement,
                    cancellation: agent_cancellation,
                    turn_cancellation,
                    operation_metadata,
                    presentation,
                    options,
                })
                .await
            {
                Ok(BrokeredApprovalOutcome::Selected { option_id }) => {
                    ApprovalPortOutcome::Selected { option_id }
                }
                Ok(BrokeredApprovalOutcome::Cancelled) => ApprovalPortOutcome::Cancelled,
                Err(error) => {
                    tracing::error!(%error, "provider permission request could not be brokered");
                    ApprovalPortOutcome::Cancelled
                }
            }
        })
    }

    fn cancel_all(
        &self,
        context: Option<Self::Context>,
        reason: &'static str,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async move {
            let Some(context) = context else { return };
            if let Some(broker) = self.broker().await
                && let Err(error) = broker.cancel_all_for_session(&context.target, reason).await
            {
                tracing::error!(%error, "failed to cancel provider permissions for turn");
            }
        })
    }

    fn cancel_retired(&self) -> InteractionFuture<'_, ()> {
        Box::pin(async move {
            if let Some(broker) = self.broker().await
                && let Err(error) = broker.cancel_retired_external().await
            {
                tracing::error!(%error, "failed to cancel provider permissions after connection loss");
            }
        })
    }
}

fn broker_presentation(
    presentation: acp_client_runtime::ApprovalPresentation,
) -> ApprovalPresentation {
    ApprovalPresentation {
        tool_name: presentation.tool_name,
        title: presentation.title,
        kind: presentation.kind,
        arguments: presentation
            .arguments
            .into_iter()
            .map(|argument| ApprovalArgument {
                name: argument.name,
                value: argument.value,
            })
            .collect(),
        permission_details: presentation.permission_details,
    }
}

fn broker_options(
    options: Vec<acp_client_runtime::ExternalApprovalOption>,
) -> Vec<ExternalApprovalOption> {
    options
        .into_iter()
        .map(|option| ExternalApprovalOption {
            option_id: option.option_id,
            label: option.label,
            scope: match option.scope {
                acp_client_runtime::ExternalApprovalOptionScope::AllowOnce => {
                    ExternalApprovalOptionScope::AllowOnce
                }
                acp_client_runtime::ExternalApprovalOptionScope::AllowAlways => {
                    ExternalApprovalOptionScope::AllowAlways
                }
                acp_client_runtime::ExternalApprovalOptionScope::RejectOnce => {
                    ExternalApprovalOptionScope::RejectOnce
                }
                acp_client_runtime::ExternalApprovalOptionScope::RejectAlways => {
                    ExternalApprovalOptionScope::RejectAlways
                }
                acp_client_runtime::ExternalApprovalOptionScope::Unsupported { provider_kind } => {
                    ExternalApprovalOptionScope::Unsupported { provider_kind }
                }
            },
        })
        .collect()
}

/// PR 2 stand-in. PR 3 composes this port with the SessionEventHub.
pub(crate) struct NoopSessionEventSink;

impl SessionEventSink for NoopSessionEventSink {
    fn publish(
        &self,
        _session_id: &str,
        _event: session_event_model::SessionEvent,
    ) -> Result<(), acp_client_runtime::EventSinkOverflow> {
        Ok(())
    }
}
