//! Propagate a Router turn cancellation to pending provider permissions.

use super::{ExternalProviderRuntime, ExternalProviderRuntimeError, ProviderCommand};
use collaboration_protocol::{OperationId, SessionRef};
use collaboration_service::ServiceApprovalBroker;
use std::sync::{Arc, Weak};
use tokio::sync::RwLock;

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
        let approval_target = self.approval_contexts.lock().ok().and_then(|contexts| {
            contexts
                .get(&provider_session_id)
                .map(|context| context.target.clone())
        });
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
        cancel_pending_approvals(&self.approval_broker, approval_target.as_ref()).await;
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
