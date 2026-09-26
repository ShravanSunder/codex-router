//! Connection-task ownership and lifecycle for provider permission requests.

use super::{
    ExternalProviderApprovalContext,
    approval_presentation::approval_presentation,
    external_permission_options::{
        ExternalPermissionOptionMapping, map_external_permission_options,
    },
};
use agent_client_protocol::schema::v1::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome,
};
use agent_client_protocol::{Agent, ConnectionTo, Error, Responder};
use collaboration_service::{
    ExternalApprovalOperationMetadata, ExternalApprovalRefusal, ExternalApprovalRequest,
    ServiceApprovalBroker,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) fn spawn_external_approval_dispatch(
    request: RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
    connection: ConnectionTo<Agent>,
    broker: Option<Arc<ServiceApprovalBroker>>,
    context: Option<ExternalProviderApprovalContext>,
    #[cfg(test)] permission_outcome: Arc<std::sync::atomic::AtomicU8>,
) -> Result<(), Error> {
    let request_cancellation = responder.cancellation();
    connection.spawn(async move {
        let outcome = match (broker, context) {
            (Some(broker), Some(context)) => {
                handle_contextual_permission_request(
                    &broker,
                    context,
                    request,
                    request_cancellation,
                )
                .await
            }
            _ => RequestPermissionOutcome::Cancelled,
        };
        #[cfg(test)]
        permission_outcome.store(
            if matches!(outcome, RequestPermissionOutcome::Selected(_)) {
                2
            } else {
                1
            },
            std::sync::atomic::Ordering::Relaxed,
        );
        responder.respond(RequestPermissionResponse::new(outcome))
    })
}

async fn handle_contextual_permission_request(
    broker: &ServiceApprovalBroker,
    context: ExternalProviderApprovalContext,
    request: RequestPermissionRequest,
    request_cancellation: agent_client_protocol::RequestCancellation,
) -> RequestPermissionOutcome {
    let presentation = approval_presentation(&request.tool_call.fields);
    let operation_metadata = ExternalApprovalOperationMetadata {
        operation_id: context.operation_id.clone(),
        target: context.target.clone(),
        binding_generation: context.binding_generation.clone(),
        method: "session/request_permission",
    };
    let options = match map_external_permission_options(request.options) {
        ExternalPermissionOptionMapping::Mapped(options) => options,
        ExternalPermissionOptionMapping::Refused { reason, options } => {
            if let Err(error) = broker
                .record_external_refusal(ExternalApprovalRefusal {
                    requester: context.requester,
                    approver: context.approver,
                    generation: context.binding_generation,
                    operation_metadata,
                    offered_options: options,
                    presentation: Some(presentation),
                    reason: reason.to_owned(),
                })
                .await
            {
                tracing::error!(%error, "failed to record refused provider permission request");
            }
            return RequestPermissionOutcome::Cancelled;
        }
    };

    let cancellation = CancellationToken::new();
    let broker_request = broker.request_external(ExternalApprovalRequest {
        requester: context.requester,
        approver: context.approver,
        generation: context.binding_generation,
        retirement: context.binding_retirement,
        cancellation: cancellation.clone(),
        operation_metadata,
        presentation: Some(presentation),
        options,
    });
    tokio::pin!(broker_request);
    let result = tokio::select! {
        biased;
        () = request_cancellation.cancelled() => {
            cancellation.cancel();
            broker_request.await
        }
        result = &mut broker_request => result,
    };
    match result {
        Ok(collaboration_service::BrokeredApprovalOutcome::Selected { option_id }) => {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
        }
        Ok(collaboration_service::BrokeredApprovalOutcome::Cancelled) => {
            RequestPermissionOutcome::Cancelled
        }
        Err(error) => {
            tracing::error!(%error, "provider permission request could not be brokered");
            RequestPermissionOutcome::Cancelled
        }
    }
}
