//! Host-local capability report derived from ACP advertisements.

use agent_client_protocol::schema::v1::{InitializeResponse, NewSessionResponse};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProviderCapabilityReport {
    pub supports_load: bool,
    pub supports_resume: bool,
    pub supports_close: bool,
    pub supports_list: bool,
    pub supports_steering: bool,
    pub router_queue: bool,
    pub supports_cancel_queued: bool,
    pub supports_modes: bool,
    pub supports_config_options: bool,
    pub supports_elicitation: bool,
    pub supports_usage: bool,
    pub accepts_image: bool,
    pub accepts_audio: bool,
    pub accepts_embedded_resource: bool,
}

impl ProviderCapabilityReport {
    pub fn from_initialize(response: &InitializeResponse) -> Self {
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
            supports_elicitation: false,
            supports_usage: false,
            accepts_image: advertised.prompt_capabilities.image,
            accepts_audio: advertised.prompt_capabilities.audio,
            accepts_embedded_resource: advertised.prompt_capabilities.embedded_context,
        }
    }

    pub fn with_session_response(&self, response: &NewSessionResponse) -> Self {
        let mut report = self.clone();
        report.supports_modes = response.modes.is_some();
        report.supports_config_options = response.config_options.is_some();
        report.supports_steering |= advertises_steering(response.meta.as_ref());
        report
    }
}

fn advertises_steering(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> bool {
    meta.and_then(|meta| meta.get("steering"))
        .and_then(|steering| steering.get("supported"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}
