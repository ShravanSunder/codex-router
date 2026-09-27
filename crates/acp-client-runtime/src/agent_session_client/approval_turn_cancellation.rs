//! Propagate Router turn cancellation to provider permissions through the interaction port.

use super::{AgentSessionClient, ExternalProviderRuntimeError, ProviderCommand};
use crate::InteractionPort;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(crate) struct PermissionResponseBarrier {
    pending: AtomicUsize,
    drained: tokio::sync::Notify,
}

impl PermissionResponseBarrier {
    pub(crate) fn track(self: &Arc<Self>) -> PermissionResponseGuard {
        self.pending.fetch_add(1, Ordering::AcqRel);
        PermissionResponseGuard(Arc::clone(self))
    }

    async fn wait_drained(&self) {
        loop {
            let notified = self.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.pending.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}

pub(crate) struct PermissionResponseGuard(Arc<PermissionResponseBarrier>);

impl Drop for PermissionResponseGuard {
    fn drop(&mut self) {
        if self.0.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.drained.notify_waiters();
        }
    }
}

pub(crate) struct ActiveApprovalContext<P: InteractionPort> {
    pub(crate) approval: P::Context,
    pub(crate) cancelling: CancellationToken,
    pub(crate) responses: Arc<PermissionResponseBarrier>,
}

impl<P: InteractionPort> Clone for ActiveApprovalContext<P> {
    fn clone(&self) -> Self {
        Self {
            approval: self.approval.clone(),
            cancelling: self.cancelling.clone(),
            responses: Arc::clone(&self.responses),
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
                    responses: Arc::clone(&context.responses),
                })
        })
    })
}

pub(crate) struct ProviderTurnCancellation<P: InteractionPort> {
    pub(crate) cancelling: CancellationToken,
    pub(crate) approval: P::Context,
    pub(crate) interaction_port: Arc<P>,
    pub(crate) responses: Arc<PermissionResponseBarrier>,
}

impl<P: InteractionPort> ProviderTurnCancellation<P> {
    pub(crate) fn mark_cancelling(&self) {
        self.cancelling.cancel();
    }

    pub(crate) async fn settle_pending_approvals(&self) {
        self.interaction_port
            .cancel_all(self.approval.clone(), "turnCancelled")
            .await;
        self.responses.wait_drained().await;
    }
}

impl<P: InteractionPort> AgentSessionClient<P> {
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

    pub(crate) async fn cancel_prompt(
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
            turn_cancellation.settle_pending_approvals().await;
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Cancel {
                provider_session_id,
                expected_operation_id,
                reply,
            })
            .await
            .map_err(|_| self.cancel_transport_failure())?;
        result
            .await
            .map_err(|_| self.cancel_transport_failure())??;
        Ok(())
    }
}

impl<P: InteractionPort> AgentSessionClient<P> {
    fn cancel_transport_failure(&self) -> ExternalProviderRuntimeError {
        if self.sink_closed.is_cancelled() {
            ExternalProviderRuntimeError::SinkClosed
        } else {
            ExternalProviderRuntimeError::Operation("provider runtime closed".to_owned())
        }
    }
}
