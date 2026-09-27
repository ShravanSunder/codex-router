//! ACP prompt submission at the provider runtime boundary.

use super::{
    ExternalProviderPromptOutcome, ExternalProviderRuntime, ExternalProviderRuntimeError,
    ProviderCommand, ProviderPromptDispatchObservation,
};
use crate::provider_prompt_content::ProviderPromptContent;
use agent_client_protocol::schema::v1::ContentBlock;
use collaboration_protocol::OperationId;

impl ExternalProviderRuntime {
    pub async fn prompt(
        &self,
        provider_session_id: String,
        prompt: String,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_for_operation(provider_session_id, None, prompt, None)
            .await
    }

    pub(super) async fn prompt_for_operation(
        &self,
        provider_session_id: String,
        operation_id: Option<OperationId>,
        prompt: String,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_content(
            provider_session_id,
            operation_id,
            vec![ContentBlock::Text(
                agent_client_protocol::schema::v1::TextContent::new(prompt),
            )],
            dispatch,
        )
        .await
    }

    pub(crate) async fn prompt_content(
        &self,
        provider_session_id: String,
        operation_id: Option<OperationId>,
        blocks: Vec<ContentBlock>,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        let capabilities = self.capability_report(&provider_session_id).await;
        let prompt = match ProviderPromptContent::new(blocks, &capabilities) {
            Ok(prompt) => prompt,
            Err(error) => {
                if let Some(dispatch) = dispatch {
                    let _result = dispatch.send(ProviderPromptDispatchObservation::NotSubmitted);
                }
                return Err(error);
            }
        };
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Prompt {
                provider_session_id,
                operation_id,
                prompt,
                dispatch,
                reply,
            })
            .await
            .map_err(|error| {
                if let ProviderCommand::Prompt {
                    dispatch: Some(dispatch),
                    ..
                } = error.0
                {
                    let _result = dispatch.send(ProviderPromptDispatchObservation::NotSubmitted);
                }
                self.prompt_transport_failure()
            })?;
        result.await.map_err(|_| self.prompt_transport_failure())?
    }

    fn prompt_transport_failure(&self) -> ExternalProviderRuntimeError {
        if self.frame_observation.limit_was_exceeded() {
            ExternalProviderRuntimeError::FrameLimitExceeded
        } else {
            ExternalProviderRuntimeError::TransportFailure
        }
    }
}
