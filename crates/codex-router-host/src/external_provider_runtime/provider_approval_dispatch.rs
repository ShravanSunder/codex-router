//! Approval context lifetime for a dispatched provider prompt.

use super::{
    ApprovalContextGuard, ExternalProviderApprovalContext, ExternalProviderPromptOutcome,
    ExternalProviderRuntime, ExternalProviderRuntimeError, ProviderPromptDispatchObservation,
};
use std::sync::Arc;

impl ExternalProviderRuntime {
    pub(crate) async fn prompt_with_approval_dispatch(
        &self,
        provider_session_id: String,
        prompt: String,
        context: ExternalProviderApprovalContext,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        let operation_id = context.operation_id.clone();
        {
            let mut contexts = self.approval_contexts.lock().map_err(|_| {
                ExternalProviderRuntimeError::Operation(
                    "provider approval context unavailable".to_owned(),
                )
            })?;
            if contexts.contains_key(&provider_session_id) {
                return Err(ExternalProviderRuntimeError::LocalBusy);
            }
            contexts.insert(provider_session_id.clone(), context);
        }
        let _context_guard = ApprovalContextGuard {
            contexts: Arc::clone(&self.approval_contexts),
            provider_session_id: provider_session_id.clone(),
            operation_id: operation_id.clone(),
        };
        self.prompt_for_operation(provider_session_id, Some(operation_id), prompt, dispatch)
            .await
    }
}
