//! Capability report derived from ACP advertisements.

use agent_client_protocol::schema::v1::{InitializeResponse, NewSessionResponse};
use session_event_model::{
    CapabilityReport, PromptContentCapabilities, ProviderAuthStatus, QueueCapability, QueueSupport,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderCapabilityReport {
    pub supports_load: bool,
    pub supports_resume: bool,
    pub supports_close: bool,
    pub supports_list: bool,
    pub supports_steering: bool,
    pub router_queue: bool,
    pub supports_cancel_queued: bool,
    pub supports_modes: bool,
    pub supports_config_options: bool,
    /// Router advertises ACP form elicitation and can route this Session's
    /// Questions to an answerer. This is a client/back-door capability, not
    /// an advertisement made by the agent.
    pub supports_elicitation: bool,
    pub supports_usage: bool,
    pub accepts_image: bool,
    pub accepts_audio: bool,
    pub accepts_embedded_resource: bool,
    pub auth_status: ProviderAuthStatus,
}

impl ProviderCapabilityReport {
    pub(crate) fn from_initialize(response: &InitializeResponse) -> Self {
        let advertised = &response.agent_capabilities;
        Self {
            supports_load: advertised.load_session,
            supports_resume: advertised.session_capabilities.resume.is_some(),
            supports_close: advertised.session_capabilities.close.is_some(),
            supports_list: advertised.session_capabilities.list.is_some(),
            supports_steering: advertises_steering(response.meta.as_ref()),
            router_queue: true,
            supports_cancel_queued: true,
            supports_modes: false,
            supports_config_options: false,
            supports_elicitation: true,
            supports_usage: false,
            accepts_image: advertised.prompt_capabilities.image,
            accepts_audio: advertised.prompt_capabilities.audio,
            accepts_embedded_resource: advertised.prompt_capabilities.embedded_context,
            auth_status: ProviderAuthStatus::NotReported,
        }
    }

    pub(crate) fn with_session_response(&self, response: &NewSessionResponse) -> Self {
        let mut report = self.clone();
        report.supports_modes = response.modes.is_some();
        report.supports_config_options = response.config_options.is_some();
        report.supports_steering |= advertises_steering(response.meta.as_ref());
        report
    }

    pub(crate) fn with_auth_status(&self, auth_status: ProviderAuthStatus) -> Self {
        let mut report = self.clone();
        report.auth_status = auth_status;
        report
    }

    pub fn to_session_model(&self) -> CapabilityReport {
        CapabilityReport {
            load: self.supports_load,
            resume: self.supports_resume,
            close: self.supports_close,
            list: self.supports_list,
            steer: self.supports_steering,
            queue: self.router_queue.then_some(QueueSupport {
                kind: QueueCapability::Router,
                can_cancel: self.supports_cancel_queued,
            }),
            modes: self.supports_modes,
            config_options: self.supports_config_options,
            elicitation: self.supports_elicitation,
            usage: self.supports_usage,
            prompt_content: PromptContentCapabilities {
                image: self.accepts_image,
                audio: self.accepts_audio,
                embedded_context: self.accepts_embedded_resource,
            },
            auth_status: self.auth_status.clone(),
        }
    }
}

fn advertises_steering(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> bool {
    meta.and_then(|meta| meta.get("steering"))
        .and_then(|steering| steering.get("supported"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::ProtocolVersion;
    use session_event_model::ProviderAuthStatus;

    /// Oracle: specification E13/R23 keeps connection auth status separate
    /// from Session state and reports silence as notReported.
    #[test]
    fn connection_auth_status_starts_unreported() {
        assert_eq!(
            ProviderCapabilityReport::default().auth_status,
            ProviderAuthStatus::NotReported
        );
    }

    /// The initialize wire fixture proves Router advertises form elicitation;
    /// its Session report must expose that back-door ability to answer Questions.
    #[test]
    fn advertised_form_elicitation_is_reported_for_each_session() {
        let response = InitializeResponse::new(ProtocolVersion::V1);
        let report = ProviderCapabilityReport::from_initialize(&response);
        assert!(report.supports_elicitation);
        assert!(report.to_session_model().elicitation);
    }
}
