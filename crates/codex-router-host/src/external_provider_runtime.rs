//! Host composition for the extracted ACP client.

#[cfg(test)]
use std::time::Duration;
use std::{path::PathBuf, sync::Arc};

use acp_client_runtime::AgentSessionClient;
pub(crate) use acp_client_runtime::ProviderPromptDispatchObservation;
use collaboration_protocol::{
    CodexGeneration, OperationId, ProviderIdentity, ProviderPromptStopReason, SessionRef,
};
use collaboration_service::ProviderSessionEventHub;
use collaboration_service::ServiceInteractionBroker;
use message_board::SessionEndpointRef;
use tokio_util::sync::CancellationToken;

use crate::acp_interaction_port::{HostInteractionPort, NoopSessionEventSink};
use crate::provider_session_event_sink::HubSessionEventSink;
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
    pub requester: ProviderIdentity,
    pub approver: ProviderIdentity,
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
        session_event_model::StopReason::Unknown(value) => {
            let safe_value = if !value.is_empty()
                && value.len() <= 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            {
                value
            } else {
                "unrecognized".to_owned()
            };
            ProviderPromptStopReason::Unknown(safe_value)
        }
    };
    Ok(ExternalProviderPromptOutcome {
        output: outcome.output,
        stop_reason,
        permission_refusal_reason: outcome.permission_refusal_reason,
    })
}

#[cfg(test)]
mod prompt_projection_tests {
    use super::*;

    #[test]
    fn unknown_stop_reason_keeps_output_and_safe_value() {
        let outcome = acp_client_runtime::ExternalProviderPromptOutcome {
            output: "known output".to_owned(),
            stop_reason: session_event_model::StopReason::Unknown("future_stop".to_owned()),
            permission_refusal_reason: None,
        };
        let projected = host_prompt_outcome(outcome).expect("unknown reason is a terminal outcome");
        assert_eq!(projected.output, "known output");
        assert_eq!(
            projected.stop_reason,
            ProviderPromptStopReason::Unknown("future_stop".into())
        );

        let untrusted = acp_client_runtime::ExternalProviderPromptOutcome {
            output: String::new(),
            stop_reason: session_event_model::StopReason::Unknown("../token=value".to_owned()),
            permission_refusal_reason: None,
        };
        assert_eq!(
            host_prompt_outcome(untrusted)
                .expect("terminal outcome")
                .stop_reason,
            ProviderPromptStopReason::Unknown("unrecognized".into())
        );
    }
}

pub struct ExternalProviderRuntime {
    client: AgentSessionClient<HostInteractionPort>,
    interaction_port: Arc<HostInteractionPort>,
    event_sink: Option<Arc<HubSessionEventSink>>,
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
            event_sink: None,
        })
    }

    pub async fn initialize_with_mcp_http(
        launch: ExternalProviderLaunch,
        model_picker: acp_client_runtime::ProviderModelPicker,
        server_name: impl Into<String>,
        server_url: impl Into<String>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        let interaction_port = Arc::new(HostInteractionPort::default());
        let client = AgentSessionClient::initialize_with_mcp_http(
            launch,
            model_picker,
            server_name,
            server_url,
            Arc::clone(&interaction_port),
            Arc::new(NoopSessionEventSink),
        )
        .await?;
        Ok(Self {
            client,
            interaction_port,
            event_sink: None,
        })
    }

    pub(crate) async fn initialize_with_mcp_http_and_hub(
        launch: ExternalProviderLaunch,
        model_picker: acp_client_runtime::ProviderModelPicker,
        server_name: impl Into<String>,
        server_url: impl Into<String>,
        hub: Arc<ProviderSessionEventHub>,
        endpoint: SessionEndpointRef,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        let interaction_port = Arc::new(HostInteractionPort::default());
        let event_sink = Arc::new(HubSessionEventSink::new(hub, endpoint));
        let published_sink: Arc<dyn acp_client_runtime::SessionEventSink> = event_sink.clone();
        interaction_port
            .install_event_sink(Arc::clone(&published_sink))
            .await;
        let result = AgentSessionClient::initialize_with_mcp_http(
            launch,
            model_picker,
            server_name,
            server_url,
            Arc::clone(&interaction_port),
            published_sink,
        )
        .await;
        let client = match result {
            Ok(client) => client,
            Err(error) => {
                let _ = event_sink.shutdown().await;
                return Err(error);
            }
        };
        Ok(Self {
            client,
            interaction_port,
            event_sink: Some(event_sink),
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
            event_sink: None,
        })
    }

    pub async fn install_approval_broker(&self, broker: Arc<ServiceInteractionBroker>) {
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

    pub(crate) fn install_catalog_refresh(
        &self,
        sender: tokio::sync::mpsc::UnboundedSender<String>,
    ) {
        if let Some(event_sink) = &self.event_sink {
            event_sink.install_catalog_refresh(sender);
        }
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
        self.create_session_with_observation(cwd)
            .await
            .map(|created| created.provider_session_id)
    }

    pub async fn create_session_with_observation(
        &self,
        cwd: PathBuf,
    ) -> Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError> {
        self.create_session_with_settings(
            cwd,
            acp_client_runtime::RequestedProviderSettings::default(),
        )
        .await
    }

    pub async fn create_session_with_settings(
        &self,
        cwd: PathBuf,
        settings: acp_client_runtime::RequestedProviderSettings,
    ) -> Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError> {
        let created = self
            .client
            .create_session_with_settings(cwd, settings)
            .await;
        let active_session = match &created {
            Ok(created) => Some(created.provider_session_id.as_str()),
            Err(ExternalProviderRuntimeError::CreatedWithoutSettings {
                provider_session_id,
                ..
            }) => Some(provider_session_id.as_str()),
            Err(ExternalProviderRuntimeError::InvalidSetting {
                provider_session_id,
                disposition: acp_client_runtime::InvalidSettingSessionDisposition::RemainsCreated,
                ..
            }) => Some(provider_session_id.as_str()),
            _ => None,
        };
        if let Some(session_id) = active_session {
            self.publish_active_session(session_id).await?;
        }
        created
    }

    pub async fn settings_unresolved(&self, provider_session_id: &str) -> bool {
        self.client.settings_unresolved(provider_session_id).await
    }

    pub async fn set_setting(
        &self,
        provider_session_id: String,
        kind: acp_client_runtime::ProviderSettingKind,
        value: String,
    ) -> Result<acp_client_runtime::EffectiveProviderSettings, ExternalProviderRuntimeError> {
        self.client
            .set_setting(provider_session_id, kind, value)
            .await
    }

    pub async fn accept_session_settings(
        &self,
        provider_session_id: String,
    ) -> Result<acp_client_runtime::EffectiveProviderSettings, ExternalProviderRuntimeError> {
        self.client
            .accept_session_settings(provider_session_id)
            .await
    }

    pub async fn settings_catalog(
        &self,
        provider_session_id: &str,
    ) -> Option<acp_client_runtime::ProviderSettingsCatalog> {
        self.client.settings_catalog(provider_session_id).await
    }

    pub async fn load_session(
        &self,
        provider_session_id: String,
        cwd: PathBuf,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.client
            .load_session(provider_session_id.clone(), cwd)
            .await?;
        self.publish_active_session(&provider_session_id).await
    }

    pub async fn resume_session(
        &self,
        provider_session_id: String,
        cwd: PathBuf,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.client
            .resume_session(provider_session_id.clone(), cwd)
            .await?;
        if let Some(event_sink) = &self.event_sink {
            event_sink
                .begin_history_unavailable(&provider_session_id)
                .await
                .map_err(|_| ExternalProviderRuntimeError::HistoryReplayUnavailable)?;
        }
        self.publish_active_session(&provider_session_id).await
    }

    async fn publish_active_session(
        &self,
        provider_session_id: &str,
    ) -> Result<(), ExternalProviderRuntimeError> {
        if let Some(event_sink) = &self.event_sink {
            let capabilities = self
                .client
                .capability_report(provider_session_id)
                .await
                .to_session_model();
            acp_client_runtime::SessionEventSink::publish(
                event_sink.as_ref(),
                provider_session_id,
                session_event_model::SessionEvent::CapabilitiesChanged { capabilities },
            )
            .map_err(|_| ExternalProviderRuntimeError::SinkClosed)?;
            event_sink
                .publish_idle(provider_session_id)
                .await
                .map_err(|_| ExternalProviderRuntimeError::SinkClosed)?;
        }
        Ok(())
    }

    pub async fn close_session(
        &self,
        provider_session_id: String,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.client
            .close_session(provider_session_id.clone())
            .await?;
        if let Some(event_sink) = &self.event_sink {
            acp_client_runtime::SessionEventSink::publish(
                event_sink.as_ref(),
                &provider_session_id,
                session_event_model::SessionEvent::StateChanged {
                    state: session_event_model::SessionState::Closed,
                },
            )
            .map_err(|_| ExternalProviderRuntimeError::HistoryReplayUnavailable)?;
        }
        Ok(())
    }

    pub async fn steer_session(
        &self,
        provider_session_id: String,
        prompt: String,
    ) -> Result<crate::ProviderSteeringOutcome, ExternalProviderRuntimeError> {
        self.steer_session_with_input(
            provider_session_id,
            session_event_model::InputId::generate(),
            prompt,
        )
        .await
    }

    pub async fn steer_session_with_input(
        &self,
        provider_session_id: String,
        input_id: session_event_model::InputId,
        prompt: String,
    ) -> Result<crate::ProviderSteeringOutcome, ExternalProviderRuntimeError> {
        self.client
            .steer_contents_with_input(provider_session_id, input_id, text_contents(prompt)?)
            .await
    }

    pub async fn steer_session_contents_with_input(
        &self,
        provider_session_id: String,
        input_id: session_event_model::InputId,
        contents: Vec<session_event_model::PromptContent>,
    ) -> Result<crate::ProviderSteeringOutcome, ExternalProviderRuntimeError> {
        self.client
            .steer_contents_with_input(provider_session_id, input_id, contents)
            .await
    }

    pub async fn session_activity(
        &self,
        provider_session_id: String,
    ) -> Result<crate::ProviderSessionActivity, ExternalProviderRuntimeError> {
        self.client.session_activity(provider_session_id).await
    }

    pub async fn active_prompt_operation(
        &self,
        provider_session_id: String,
    ) -> Result<Option<OperationId>, ExternalProviderRuntimeError> {
        self.client
            .active_prompt_operation(provider_session_id)
            .await
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
        if let Some(event_sink) = &self.event_sink
            && let Err(error) = event_sink.shutdown().await
        {
            tracing::error!(%error, "provider Session event consumer failed to drain");
        }
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

    #[cfg(test)]
    pub async fn prompt(
        &self,
        provider_session_id: String,
        prompt: String,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_with_approval_context(
            provider_session_id.clone(),
            prompt,
            test_approval_context(&provider_session_id, None),
        )
        .await
    }

    pub async fn prompt_with_approval_context(
        &self,
        provider_session_id: String,
        prompt: String,
        context: ExternalProviderApprovalContext,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_with_approval_context_for_input(
            provider_session_id,
            session_event_model::InputId::generate(),
            prompt,
            context,
        )
        .await
    }

    pub async fn prompt_with_approval_context_for_input(
        &self,
        provider_session_id: String,
        input_id: session_event_model::InputId,
        prompt: String,
        context: ExternalProviderApprovalContext,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_contents_with_approval_dispatch_for_input(
            provider_session_id,
            input_id,
            text_contents(prompt)?,
            context,
            None,
        )
        .await
    }

    pub(crate) async fn prompt_contents_with_approval_dispatch_for_input(
        &self,
        provider_session_id: String,
        input_id: session_event_model::InputId,
        contents: Vec<session_event_model::PromptContent>,
        context: ExternalProviderApprovalContext,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        host_prompt_outcome(
            self.client
                .prompt_contents_with_approval_dispatch_for_input(
                    provider_session_id,
                    input_id,
                    contents,
                    context,
                    dispatch,
                )
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
        self.prompt_contents_with_approval_dispatch_for_input(
            provider_session_id.clone(),
            session_event_model::InputId::generate(),
            text_contents(prompt)?,
            test_approval_context(&provider_session_id, operation_id),
            dispatch,
        )
        .await
    }
}

fn text_contents(
    prompt: String,
) -> Result<Vec<session_event_model::PromptContent>, ExternalProviderRuntimeError> {
    session_event_model::PromptContent::text(prompt)
        .map(|content| vec![content])
        .map_err(|_| ExternalProviderRuntimeError::Operation("invalid prompt text".to_owned()))
}

#[cfg(test)]
fn test_approval_context(
    provider_session_id: &str,
    operation_id: Option<OperationId>,
) -> ExternalProviderApprovalContext {
    let service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let target: SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint": {"serviceId": service_id, "endpointId": "cursor-local"},
        "sessionId": provider_session_id,
    }))
    .expect("test provider target");
    ExternalProviderApprovalContext {
        requester: target.clone().into(),
        approver: target.clone().into(),
        target,
        operation_id: operation_id.unwrap_or_else(OperationId::generate),
        binding_generation: serde_json::from_value(serde_json::json!({
            "serviceEpoch": service_id, "generation": 1
        }))
        .expect("test generation"),
        binding_retirement: CancellationToken::new(),
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
#[cfg(test)]
#[path = "external_provider_runtime/approval_push_fixture.rs"]
mod approval_push_fixture;
