//! Connection-task ownership and lifecycle for provider permission requests.

#[cfg(any(test, feature = "test-observation"))]
use super::ExternalProviderApprovalRefusalWarning;
use super::{ActiveApprovalContext, ExternalProviderApprovalRefusalReason};
use crate::{
    ApprovalPortOutcome, InteractionPort, ProviderPersistenceTarget, RefusedApprovalOffer,
    map_permission_options, reviewed_approval_fields,
};
use agent_client_protocol::schema::v1::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome,
};
use agent_client_protocol::{Agent, ConnectionTo, Error, Responder};
use session_event_model::ApprovalRequest;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub(super) struct PermissionDispatchState<P: InteractionPort> {
    pub(super) interaction_port: Arc<P>,
    pub(super) refusal_reasons:
        Arc<Mutex<HashMap<P::OperationId, ExternalProviderApprovalRefusalReason>>>,
    pub(super) endpoint_id: Arc<RwLock<Option<String>>>,
    pub(super) persistence_target: ProviderPersistenceTarget,
    #[cfg(any(test, feature = "test-observation"))]
    pub(super) refusal_warnings: Arc<Mutex<Vec<ExternalProviderApprovalRefusalWarning>>>,
    #[cfg(any(test, feature = "test-observation"))]
    pub(super) permission_outcome: Arc<std::sync::atomic::AtomicU8>,
}

pub(super) async fn record_permission_refusal<P: InteractionPort>(
    state: &PermissionDispatchState<P>,
    provider_session_id: &str,
    operation_id: Option<P::OperationId>,
    reason: ExternalProviderApprovalRefusalReason,
) {
    let endpoint = state
        .endpoint_id
        .read()
        .await
        .clone()
        .unwrap_or_else(|| "unknown".to_owned());
    tracing::warn!(
        endpoint = %endpoint,
        provider_session_id,
        method = "session/request_permission",
        reason_code = reason.code(),
        "provider permission request refused before approval broker",
    );
    #[cfg(any(test, feature = "test-observation"))]
    if let Ok(mut warnings) = state.refusal_warnings.lock() {
        warnings.push(ExternalProviderApprovalRefusalWarning {
            endpoint,
            provider_session_id: provider_session_id.to_owned(),
            method: "session/request_permission",
            reason_code: reason,
        });
    }
    if let Some(operation_id) = operation_id
        && let Ok(mut reasons) = state.refusal_reasons.lock()
    {
        reasons.insert(operation_id, reason);
    }
}

pub(super) fn spawn_external_approval_dispatch<P: InteractionPort>(
    request: RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
    connection: ConnectionTo<Agent>,
    context: Option<ActiveApprovalContext<P>>,
    state: PermissionDispatchState<P>,
) -> Result<(), Error> {
    let request_id = serde_json::to_string(&(request.session_id.0.as_ref(), responder.id()))
        .map_err(|_| Error::internal_error())?;
    let request_cancellation = responder.cancellation();
    connection.spawn(async move {
        let outcome = match context {
            Some(context) => {
                handle_contextual_permission_request(
                    context,
                    request,
                    request_id,
                    request_cancellation,
                    &state,
                )
                .await
            }
            None => RequestPermissionOutcome::Cancelled,
        };
        #[cfg(any(test, feature = "test-observation"))]
        state.permission_outcome.store(
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

async fn handle_contextual_permission_request<P: InteractionPort>(
    context: ActiveApprovalContext<P>,
    request: RequestPermissionRequest,
    request_id: String,
    request_cancellation: agent_client_protocol::RequestCancellation,
    state: &PermissionDispatchState<P>,
) -> RequestPermissionOutcome {
    let provider_session_id = request.session_id.0.to_string();
    let reviewed = reviewed_approval_fields(&request.tool_call);
    let options = match map_permission_options(request.options, state.persistence_target) {
        Ok(options) => options,
        Err(refused) => {
            state
                .interaction_port
                .record_refusal(
                    context.approval,
                    RefusedApprovalOffer {
                        request_id,
                        reason: refused.reason,
                        title: reviewed.title,
                        description: reviewed.description,
                        subject: reviewed.subject,
                        options: refused.options,
                    },
                )
                .await;
            return RequestPermissionOutcome::Cancelled;
        }
    };
    let canonical_request = ApprovalRequest {
        request_id,
        title: reviewed.title,
        description: reviewed.description,
        subject: reviewed.subject,
        options_origin: session_event_model::OptionsOrigin::AgentOffered,
        options,
    };
    let agent_cancellation = CancellationToken::new();
    let approval = state.interaction_port.request_approval(
        context.approval.clone(),
        canonical_request,
        context.cancelling,
        agent_cancellation.clone(),
    );
    tokio::pin!(approval);
    let outcome = tokio::select! {
        biased;
        () = request_cancellation.cancelled() => {
            agent_cancellation.cancel();
            approval.await
        }
        outcome = &mut approval => outcome,
    };
    match outcome {
        ApprovalPortOutcome::Selected { option_id, .. } => {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
        }
        ApprovalPortOutcome::Cancelled => RequestPermissionOutcome::Cancelled,
        ApprovalPortOutcome::Unavailable => {
            record_permission_refusal(
                state,
                &provider_session_id,
                Some(P::operation_id(&context.approval)),
                ExternalProviderApprovalRefusalReason::ApprovalBrokerUnavailable,
            )
            .await;
            RequestPermissionOutcome::Cancelled
        }
    }
}
