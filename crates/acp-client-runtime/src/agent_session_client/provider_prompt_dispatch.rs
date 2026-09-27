//! ACP prompt submission at the provider runtime boundary.

use super::{
    AgentSessionClient, ExternalProviderPromptOutcome, ExternalProviderRuntimeError,
    ProviderCommand, ProviderPromptDispatchObservation,
};
use crate::InteractionPort;
use crate::provider_prompt_content::ProviderPromptContent;
use agent_client_protocol::schema::v1::ContentBlock;
use session_event_model::InputId;

impl<P: InteractionPort> AgentSessionClient<P> {
    pub(crate) async fn prompt_content(
        &self,
        provider_session_id: String,
        input_id: InputId,
        operation_id: Option<P::OperationId>,
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
                input_id,
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
        if self.sink_closed.is_cancelled() {
            ExternalProviderRuntimeError::SinkClosed
        } else if self.frame_observation.limit_was_exceeded() {
            ExternalProviderRuntimeError::FrameLimitExceeded
        } else {
            ExternalProviderRuntimeError::TransportFailure
        }
    }
}
