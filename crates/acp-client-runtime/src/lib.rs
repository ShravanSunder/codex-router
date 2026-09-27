//! ACP client transport and provider Session ownership.
//!
//! ACP SDK values are decoded inside this crate, not exposed as default client APIs.
//!
//! ```compile_fail
//! let _ = acp_client_runtime::ProviderCapabilityReport::from_initialize;
//! ```
//!
//! ```compile_fail
//! let _ = acp_client_runtime::ProviderPromptContent::new;
//! ```
//!
//! ```compile_fail
//! let _ = acp_client_runtime::acp_operation_error;
//! ```
//!
//! ```compile_fail
//! let _ = acp_client_runtime::sanitized_initialization_error;
//! ```
//!
//! ```compile_fail
//! let _ = acp_client_runtime::approval_presentation;
//! ```
//!
//! ```compile_fail
//! let _ = acp_client_runtime::map_external_permission_options;
//! ```
//!
//! ```compile_fail
//! let _ = acp_client_runtime::ExternalProviderRuntimeError::UnsupportedProtocol {
//!     actual: agent_client_protocol::schema::ProtocolVersion::V0,
//! };
//! ```

mod acp_protocol_version;
mod agent_session_client;
mod approval_presentation;
mod external_permission_options;
mod interaction_port;
mod provider_capability_report;
mod provider_connection_activity;
mod provider_item_projection;
mod provider_persistence_target;
mod provider_prompt_content;
mod provider_prompt_observation;
mod provider_prompt_result_codec;
mod provider_session_actor;
mod provider_session_setting_update;
mod provider_session_settings;
mod provider_settings_catalog_codec;
mod provider_update_kind;
mod session_event_sink;

pub use acp_protocol_version::AcpProtocolVersion;
pub use agent_session_client::*;
pub(crate) use approval_presentation::reviewed_approval_fields;
pub use external_permission_options::RefusedPermissionOption;
pub(crate) use external_permission_options::map_permission_options;
pub use interaction_port::{
    ApprovalPortOutcome, InteractionFuture, InteractionPort, RefusedApprovalOffer,
};
pub use provider_capability_report::ProviderCapabilityReport;
pub use provider_persistence_target::ProviderPersistenceTarget;
pub use provider_prompt_content::ProviderPromptContent;
pub use provider_session_actor::{
    ProviderPromptDispatchObservation, ProviderSessionActivity, ProviderSteeringOutcome,
};
pub use provider_session_settings::{
    AppliedProviderSetting, EffectiveProviderSettings, FailedProviderSetting,
    InvalidSettingSessionDisposition, ProviderConfigOption, ProviderConfigValue,
    ProviderSettingChoice, ProviderSettingKind, ProviderSettingsCatalog, RequestedProviderSettings,
};
pub use session_event_sink::{
    EventSinkOverflow, HistoryReplayFuture, HistoryReplayUnavailable, SessionEventSink,
};

#[cfg(feature = "test-observation")]
pub const MAX_ACP_FRAME_BYTES: usize = agent_session_client::MAX_ACP_FRAME_BYTES;

#[cfg(feature = "test-observation")]
pub fn acp_operation_error_for_test(
    error: agent_client_protocol::Error,
) -> ExternalProviderRuntimeError {
    agent_session_client::acp_operation_error(error)
}

#[cfg(feature = "test-observation")]
pub fn sanitized_initialization_error_for_test(error: &agent_client_protocol::Error) -> String {
    agent_session_client::sanitized_initialization_error(error)
}

#[cfg(feature = "test-observation")]
pub fn sanitized_acp_error_for_test(
    error: &agent_client_protocol::Error,
    method: &str,
    stage: &str,
) -> String {
    agent_session_client::sanitized_acp_error(error, method, stage)
}
