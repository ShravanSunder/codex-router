//! Host composition for the extracted ACP client.

#[cfg(test)]
use std::time::Duration;
use std::{path::PathBuf, sync::Arc};

use acp_client_runtime::AgentSessionClient;
pub(crate) use acp_client_runtime::ProviderPromptDispatchObservation;
use collaboration_protocol::{CodexGeneration, OperationId, ProviderPromptStopReason, SessionRef};
use collaboration_service::ServiceApprovalBroker;
use tokio_util::sync::CancellationToken;

use crate::acp_interaction_port::{HostInteractionPort, NoopSessionEventSink};
#[cfg(test)]
use crate::{ProviderSessionActivity, ProviderSteeringOutcome};
#[cfg(test)]
use acp_client_runtime::{
    acp_operation_error_for_test as acp_operation_error,
    sanitized_acp_error_for_test as sanitized_acp_error,
};

pub use acp_client_runtime::{
    ExternalProviderAdmission, ExternalProviderApprovalRefusalReason,
    ExternalProviderCreatedSession, ExternalProviderLaunch, ExternalProviderRuntimeError,
};
#[cfg(test)]
pub use acp_client_runtime::{
    ExternalProviderApprovalRefusalWarning, ExternalProviderPermissionObservation,
    ExternalProviderPermissionOutcome, ExternalProviderToolCall, ExternalProviderToolOutcome,
};

#[derive(Clone, Debug)]
pub struct ExternalProviderApprovalContext {
    pub requester: SessionRef,
    pub approver: SessionRef,
    pub target: SessionRef,
    pub operation_id: OperationId,
    pub binding_generation: CodexGeneration,
    pub binding_retirement: CancellationToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderPromptOutcome {
    pub output: String,
    pub stop_reason: ProviderPromptStopReason,
    pub permission_refusal_reason: Option<ExternalProviderApprovalRefusalReason>,
}

fn host_prompt_outcome(
    outcome: acp_client_runtime::ExternalProviderPromptOutcome,
) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
    let stop_reason = match outcome.stop_reason {
        session_event_model::StopReason::EndTurn => ProviderPromptStopReason::EndTurn,
        session_event_model::StopReason::MaxTokens => ProviderPromptStopReason::MaxTokens,
        session_event_model::StopReason::MaxTurnRequests => {
            ProviderPromptStopReason::MaxTurnRequests
        }
        session_event_model::StopReason::Refusal => ProviderPromptStopReason::Refusal,
        session_event_model::StopReason::Cancelled => ProviderPromptStopReason::Cancelled,
        session_event_model::StopReason::Unknown(_) => {
            return Err(ExternalProviderRuntimeError::UnknownStopReason {
                suffix: String::new(),
            });
        }
    };
    Ok(ExternalProviderPromptOutcome {
        output: outcome.output,
        stop_reason,
        permission_refusal_reason: outcome.permission_refusal_reason,
    })
}

pub struct ExternalProviderRuntime {
    client: AgentSessionClient<HostInteractionPort>,
    interaction_port: Arc<HostInteractionPort>,
}

impl std::fmt::Debug for ExternalProviderRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalProviderRuntime")
            .field("admission", self.client.admission())
            .finish_non_exhaustive()
    }
}

impl ExternalProviderRuntime {
    pub async fn initialize(
        launch: ExternalProviderLaunch,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        let interaction_port = Arc::new(HostInteractionPort::default());
        let client = AgentSessionClient::initialize(
            launch,
            Arc::clone(&interaction_port),
            Arc::new(NoopSessionEventSink),
        )
        .await?;
        Ok(Self {
            client,
            interaction_port,
        })
    }

    pub async fn initialize_with_mcp_http(
        launch: ExternalProviderLaunch,
        server_name: impl Into<String>,
        server_url: impl Into<String>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        let interaction_port = Arc::new(HostInteractionPort::default());
        let client = AgentSessionClient::initialize_with_mcp_http(
            launch,
            server_name,
            server_url,
            Arc::clone(&interaction_port),
            Arc::new(NoopSessionEventSink),
        )
        .await?;
        Ok(Self {
            client,
            interaction_port,
        })
    }

    #[cfg(test)]
    async fn initialize_with_timeout(
        launch: ExternalProviderLaunch,
        initialize_timeout: Duration,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        let interaction_port = Arc::new(HostInteractionPort::default());
        let client = AgentSessionClient::initialize_with_timeout(
            launch,
            initialize_timeout,
            Arc::clone(&interaction_port),
            Arc::new(NoopSessionEventSink),
        )
        .await?;
        Ok(Self {
            client,
            interaction_port,
        })
    }

    pub async fn install_approval_broker(&self, broker: Arc<ServiceApprovalBroker>) {
        self.interaction_port.install_broker(broker).await;
    }

    pub fn admission(&self) -> &ExternalProviderAdmission {
        self.client.admission()
    }

    pub(crate) async fn capability_report(
        &self,
        provider_session_id: &str,
    ) -> acp_client_runtime::ProviderCapabilityReport {
        self.client.capability_report(provider_session_id).await
    }

    pub fn retirement(&self) -> CancellationToken {
        self.client.retirement()
    }

    pub async fn set_endpoint_id(&self, endpoint_id: String) {
        self.client.set_endpoint_id(endpoint_id).await;
    }

    pub async fn create_session(
        &self,
        cwd: PathBuf,
    ) -> Result<String, ExternalProviderRuntimeError> {
        self.client.create_session(cwd).await
    }

    pub async fn create_session_with_observation(
        &self,
        cwd: PathBuf,
    ) -> Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError> {
        self.client.create_session_with_observation(cwd).await
    }

    pub async fn load_session(
        &self,
        provider_session_id: String,
        cwd: PathBuf,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.client.load_session(provider_session_id, cwd).await
    }

    pub async fn steer_session(
        &self,
        provider_session_id: String,
        prompt: String,
    ) -> Result<crate::ProviderSteeringOutcome, ExternalProviderRuntimeError> {
        self.client.steer_session(provider_session_id, prompt).await
    }

    pub async fn session_activity(
        &self,
        provider_session_id: String,
    ) -> Result<crate::ProviderSessionActivity, ExternalProviderRuntimeError> {
        self.client.session_activity(provider_session_id).await
    }

    pub async fn wait_session_idle(
        &self,
        provider_session_id: String,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.client.wait_session_idle(provider_session_id).await
    }

    pub async fn cancel_prompt_operation(
        &self,
        provider_session_id: String,
        operation_id: OperationId,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.client
            .cancel_prompt_operation(provider_session_id, operation_id)
            .await
    }

    #[cfg(test)]
    pub async fn cancel_active_prompt(
        &self,
        provider_session_id: String,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.client.cancel_active_prompt(provider_session_id).await
    }

    pub async fn shutdown(&self) {
        self.client.shutdown().await;
    }

    pub fn shutdown_failed(&self) -> bool {
        self.client.shutdown_failed()
    }

    #[cfg(test)]
    pub fn permission_observation(&self) -> ExternalProviderPermissionObservation {
        self.client.permission_observation()
    }

    #[cfg(test)]
    pub fn approval_refusal_warnings(&self) -> Vec<ExternalProviderApprovalRefusalWarning> {
        self.client.approval_refusal_warnings()
    }

    #[cfg(test)]
    fn take_test_tool_calls(&self) -> Vec<ExternalProviderToolCall> {
        self.client.take_test_tool_calls()
    }

    #[cfg(test)]
    fn active_approval_operation(&self, provider_session_id: &str) -> Option<OperationId> {
        self.client.active_approval_operation(provider_session_id)
    }

    #[cfg(test)]
    fn active_approval_count(&self) -> usize {
        self.client.active_approval_count()
    }

    #[cfg(test)]
    async fn abort_owner_for_test(&self) {
        self.client.abort_owner_for_test().await;
    }

    pub async fn prompt(
        &self,
        provider_session_id: String,
        prompt: String,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        host_prompt_outcome(self.client.prompt(provider_session_id, prompt).await?)
    }

    pub async fn prompt_with_approval_context(
        &self,
        provider_session_id: String,
        prompt: String,
        context: ExternalProviderApprovalContext,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        host_prompt_outcome(
            self.client
                .prompt_with_approval_context(provider_session_id, prompt, context)
                .await?,
        )
    }

    pub(crate) async fn prompt_with_approval_dispatch(
        &self,
        provider_session_id: String,
        prompt: String,
        context: ExternalProviderApprovalContext,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        host_prompt_outcome(
            self.client
                .prompt_with_approval_dispatch(provider_session_id, prompt, context, dispatch)
                .await?,
        )
    }

    #[cfg(test)]
    async fn prompt_for_operation(
        &self,
        provider_session_id: String,
        operation_id: Option<OperationId>,
        prompt: String,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        host_prompt_outcome(
            self.client
                .prompt_for_operation(provider_session_id, operation_id, prompt, dispatch)
                .await?,
        )
    }
}

#[cfg(test)]
#[path = "external_provider_runtime/fixture_tests.rs"]
mod fixture_tests;
#[cfg(test)]
use fixture_tests::*;

#[cfg(test)]
#[path = "external_provider_runtime/live_tests.rs"]
mod live_tests;
#[cfg(test)]
#[path = "external_provider_runtime/robustness_tests.rs"]
mod robustness_tests;
#[cfg(test)]
#[path = "external_provider_runtime/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "external_provider_runtime/acp_scripted_fixture.rs"]
pub(crate) mod acp_scripted_fixture;
#[cfg(test)]
#[path = "external_provider_runtime/approval_dispatch_tests.rs"]
mod approval_dispatch_tests;
