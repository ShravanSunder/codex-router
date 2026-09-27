//! Approval context lifetime for a dispatched provider prompt.

use super::{
    ActiveApprovalContext, AgentSessionClient, ApprovalContextGuard, ExternalProviderPromptOutcome,
    ExternalProviderRuntimeError, ProviderPromptDispatchObservation,
};
use crate::provider_prompt_content::acp_blocks_from_prompt_content;
use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
use session_event_model::{InputId, PromptContent};
use std::sync::Arc;

impl<P: crate::InteractionPort> AgentSessionClient<P> {
    pub async fn prompt_with_approval_dispatch(
        &self,
        provider_session_id: String,
        prompt: String,
        context: P::Context,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_with_approval_dispatch_for_input(
            provider_session_id,
            InputId::generate(),
            prompt,
            context,
            dispatch,
        )
        .await
    }

    pub async fn prompt_with_approval_dispatch_for_input(
        &self,
        provider_session_id: String,
        input_id: InputId,
        prompt: String,
        context: P::Context,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_with_approval_dispatch_blocks_for_input(
            provider_session_id,
            input_id,
            vec![ContentBlock::Text(TextContent::new(prompt))],
            context,
            dispatch,
        )
        .await
    }

    /// Preserve the typed approval context for a multi-block delivery turn.
    pub async fn prompt_contents_with_approval_dispatch_for_input(
        &self,
        provider_session_id: String,
        input_id: InputId,
        contents: Vec<PromptContent>,
        context: P::Context,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_with_approval_dispatch_blocks_for_input(
            provider_session_id,
            input_id,
            acp_blocks_from_prompt_content(contents),
            context,
            dispatch,
        )
        .await
    }

    async fn prompt_with_approval_dispatch_blocks_for_input(
        &self,
        provider_session_id: String,
        input_id: InputId,
        blocks: Vec<ContentBlock>,
        context: P::Context,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        let operation_id = P::operation_id(&context);
        let binding_retirement = P::binding_retirement(&context);
        {
            let mut contexts = self.approval_contexts.lock().map_err(|_| {
                ExternalProviderRuntimeError::Operation(
                    "provider approval context unavailable".to_owned(),
                )
            })?;
            if contexts.contains_key(&provider_session_id) {
                return Err(ExternalProviderRuntimeError::LocalBusy);
            }
            contexts.insert(
                provider_session_id.clone(),
                ActiveApprovalContext {
                    approval: context,
                    cancelling: tokio_util::sync::CancellationToken::new(),
                },
            );
        }
        let _context_guard = ApprovalContextGuard {
            contexts: Arc::clone(&self.approval_contexts),
            provider_session_id: provider_session_id.clone(),
            operation_id: operation_id.clone(),
        };
        let prompt_result = self
            .prompt_content(
                provider_session_id,
                input_id,
                Some(operation_id.clone()),
                blocks,
                dispatch,
            )
            .await;
        if binding_retirement.is_cancelled() {
            return Err(ExternalProviderRuntimeError::TransportFailure);
        }
        let mut outcome = prompt_result?;
        outcome.permission_refusal_reason = self
            .permission_refusal_reasons
            .lock()
            .ok()
            .and_then(|mut refusals| refusals.remove(&operation_id));
        Ok(outcome)
    }
}
