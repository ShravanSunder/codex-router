//! Propagate a Router turn cancellation to pending provider permissions.

use super::{
    ExternalProviderApprovalContext, ExternalProviderRuntime, ExternalProviderRuntimeError,
    ProviderCommand,
};
use collaboration_protocol::{OperationId, SessionRef};
use collaboration_service::ServiceApprovalBroker;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) struct ActiveApprovalContext {
    pub(crate) approval: ExternalProviderApprovalContext,
    pub(crate) cancelling: CancellationToken,
    pub(crate) response_gate: Arc<Mutex<bool>>,
}

pub(crate) fn active_turn_cancellation(
    contexts: &Arc<Mutex<HashMap<String, ActiveApprovalContext>>>,
    approval_broker: &Arc<RwLock<Option<Weak<ServiceApprovalBroker>>>>,
    provider_session_id: &str,
    operation_id: Option<&OperationId>,
) -> Option<ProviderTurnCancellation> {
    contexts.lock().ok().and_then(|contexts| {
        contexts.get(provider_session_id).and_then(|context| {
            (operation_id.is_none() || Some(&context.approval.operation_id) == operation_id).then(
                || ProviderTurnCancellation {
                    cancelling: context.cancelling.clone(),
                    response_gate: Arc::clone(&context.response_gate),
                    target: context.approval.target.clone(),
                    approval_broker: Arc::clone(approval_broker),
                },
            )
        })
    })
}

pub(crate) struct ProviderTurnCancellation {
    pub(crate) cancelling: CancellationToken,
    pub(crate) response_gate: Arc<std::sync::Mutex<bool>>,
    pub(crate) target: SessionRef,
    pub(crate) approval_broker: Arc<RwLock<Option<Weak<ServiceApprovalBroker>>>>,
}

impl ProviderTurnCancellation {
    pub(crate) fn mark_cancelling(&self) {
        if let Ok(mut cancelling) = self.response_gate.lock() {
            *cancelling = true;
        }
        self.cancelling.cancel();
    }

    pub(crate) async fn settle_pending_approvals(&self) {
        cancel_pending_approvals(&self.approval_broker, Some(&self.target)).await;
    }
}

impl ExternalProviderRuntime {
    #[cfg(test)]
    pub async fn cancel_active_prompt(
        &self,
        provider_session_id: String,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.cancel_prompt(provider_session_id, None).await
    }

    pub async fn cancel_prompt_operation(
        &self,
        provider_session_id: String,
        expected_operation_id: OperationId,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.cancel_prompt(provider_session_id, Some(expected_operation_id))
            .await
    }

    async fn cancel_prompt(
        &self,
        provider_session_id: String,
        expected_operation_id: Option<OperationId>,
    ) -> Result<(), ExternalProviderRuntimeError> {
        let turn_cancellation = active_turn_cancellation(
            &self.approval_contexts,
            &self.approval_broker,
            &provider_session_id,
            expected_operation_id.as_ref(),
        );
        if let Some(turn_cancellation) = &turn_cancellation {
            turn_cancellation.mark_cancelling();
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Cancel {
                provider_session_id,
                expected_operation_id,
                reply,
            })
            .await
            .map_err(|_| {
                ExternalProviderRuntimeError::Operation("provider runtime closed".to_owned())
            })?;
        result.await.map_err(|_| {
            ExternalProviderRuntimeError::Operation("provider runtime closed".to_owned())
        })??;
        if let Some(turn_cancellation) = &turn_cancellation {
            turn_cancellation.settle_pending_approvals().await;
        }
        Ok(())
    }
}

pub(super) async fn cancel_pending_approvals(
    approval_broker: &Arc<RwLock<Option<Weak<ServiceApprovalBroker>>>>,
    target: Option<&SessionRef>,
) {
    let Some(target) = target else {
        return;
    };
    let broker = approval_broker
        .read()
        .await
        .as_ref()
        .and_then(Weak::upgrade);
    if let Some(broker) = broker
        && let Err(error) = broker
            .cancel_all_for_session(target, "turn cancelled")
            .await
    {
        tracing::error!(%error, "failed to cancel provider permissions for turn");
    }
}

pub(super) async fn cancel_on_provider_loss(
    approval_broker: &Arc<RwLock<Option<Weak<ServiceApprovalBroker>>>>,
) {
    let broker = approval_broker
        .read()
        .await
        .as_ref()
        .and_then(Weak::upgrade);
    if let Some(broker) = broker
        && let Err(error) = broker.cancel_retired_external().await
    {
        tracing::error!(%error, "failed to cancel provider permissions after connection loss");
    }
}
