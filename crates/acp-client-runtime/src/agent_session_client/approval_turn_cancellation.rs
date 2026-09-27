//! Propagate Router turn cancellation to provider permissions through the interaction port.

use super::{AgentSessionClient, ExternalProviderRuntimeError, ProviderCommand};
use crate::InteractionPort;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub(crate) struct ActiveApprovalContext<P: InteractionPort> {
    pub(crate) approval: P::Context,
    pub(crate) cancelling: CancellationToken,
}

impl<P: InteractionPort> Clone for ActiveApprovalContext<P> {
    fn clone(&self) -> Self {
        Self {
            approval: self.approval.clone(),
            cancelling: self.cancelling.clone(),
        }
    }
}

pub(crate) fn active_turn_cancellation<P: InteractionPort>(
    contexts: &Arc<Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
    interaction_port: &Arc<P>,
    provider_session_id: &str,
    operation_id: Option<&P::OperationId>,
) -> Option<ProviderTurnCancellation<P>> {
    contexts.lock().ok().and_then(|contexts| {
        contexts.get(provider_session_id).and_then(|context| {
            (operation_id.is_none() || operation_id == Some(&P::operation_id(&context.approval)))
                .then(|| ProviderTurnCancellation {
                    cancelling: context.cancelling.clone(),
                    approval: context.approval.clone(),
                    interaction_port: Arc::clone(interaction_port),
                })
        })
    })
}

pub(crate) struct ProviderTurnCancellation<P: InteractionPort> {
    pub(crate) cancelling: CancellationToken,
    pub(crate) approval: P::Context,
    pub(crate) interaction_port: Arc<P>,
}

impl<P: InteractionPort> ProviderTurnCancellation<P> {
    pub(crate) fn mark_cancelling(&self) {
        self.cancelling.cancel();
    }

    pub(crate) async fn settle_pending_approvals(&self) {
        self.interaction_port
            .cancel_all(Some(self.approval.clone()), "turnCancelled")
            .await;
    }
}

impl<P: InteractionPort> AgentSessionClient<P> {
    #[cfg(any(test, feature = "test-observation"))]
    pub async fn cancel_active_prompt(
        &self,
        provider_session_id: String,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.cancel_prompt(provider_session_id, None).await
    }

    pub async fn cancel_prompt_operation(
        &self,
        provider_session_id: String,
        expected_operation_id: P::OperationId,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.cancel_prompt(provider_session_id, Some(expected_operation_id))
            .await
    }

    async fn cancel_prompt(
        &self,
        provider_session_id: String,
        expected_operation_id: Option<P::OperationId>,
    ) -> Result<(), ExternalProviderRuntimeError> {
        let turn_cancellation = active_turn_cancellation(
            &self.approval_contexts,
            &self.interaction_port,
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
