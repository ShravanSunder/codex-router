//! Provider admission, prompt result, and safe ACP error contracts.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderLaunch {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
}

impl ExternalProviderLaunch {
    pub(super) fn sdk_config(&self) -> AcpAgentConfig {
        AcpAgentConfig::new(&self.executable)
            .args(self.arguments.clone())
            .envs(self.environment.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderAdmission {
    pub runtime_name: Option<String>,
    pub runtime_version: Option<String>,
    pub supports_load: bool,
    pub supports_mcp_http: bool,
    pub supports_steering: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderPromptOutcome {
    pub output: String,
    pub stop_reason: ProviderPromptStopReason,
    pub permission_refusal_reason: Option<ExternalProviderApprovalRefusalReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalProviderApprovalRefusalReason {
    MissingPromptContext,
    ApprovalBrokerUnavailable,
}

impl ExternalProviderApprovalRefusalReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::MissingPromptContext => "missingPromptContext",
            Self::ApprovalBrokerUnavailable => "approvalBrokerUnavailable",
        }
    }
}

#[cfg(any(test, feature = "test-observation"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderApprovalRefusalWarning {
    pub endpoint: String,
    pub provider_session_id: String,
    pub method: &'static str,
    pub reason_code: ExternalProviderApprovalRefusalReason,
}

#[cfg(any(test, feature = "test-observation"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderToolCall {
    pub name: Option<String>,
    pub title: String,
    pub kind: ToolKind,
    pub status: agent_client_protocol::schema::v1::ToolCallStatus,
    pub outcome: ExternalProviderToolOutcome,
}

#[cfg(any(test, feature = "test-observation"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalProviderToolOutcome {
    Success,
    Error,
    Rejected,
    PermissionDenied,
    Unknown,
}

#[cfg(any(test, feature = "test-observation"))]
pub(crate) fn classify_mcp_tool_outcome(
    raw_output: Option<&serde_json::Value>,
) -> ExternalProviderToolOutcome {
    let Some(output) = raw_output.and_then(serde_json::Value::as_object) else {
        return ExternalProviderToolOutcome::Unknown;
    };
    if output.get("error").is_some() {
        ExternalProviderToolOutcome::Error
    } else if output.get("rejected") == Some(&serde_json::Value::Bool(true)) {
        ExternalProviderToolOutcome::Rejected
    } else if output.get("permissionDenied") == Some(&serde_json::Value::Bool(true)) {
        ExternalProviderToolOutcome::PermissionDenied
    } else if output.get("success") == Some(&serde_json::Value::Bool(true)) {
        ExternalProviderToolOutcome::Success
    } else {
        ExternalProviderToolOutcome::Unknown
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderCreatedSession {
    pub provider_session_id: String,
}

#[cfg(any(test, feature = "test-observation"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalProviderPermissionOutcome {
    Cancelled,
    Selected,
}

#[cfg(any(test, feature = "test-observation"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalProviderPermissionObservation {
    pub method: &'static str,
    pub request_count: u64,
    pub last_outcome: Option<ExternalProviderPermissionOutcome>,
    pub execute_tool_call_count: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalProviderRuntimeError {
    #[error("provider process could not be launched: {0}")]
    Launch(String),
    #[error("provider ACP initialization failed: {0}")]
    Initialize(String),
    #[error("provider ACP initialization timed out")]
    InitializeTimeout,
    #[error("provider selected unsupported ACP protocol version {actual:?}")]
    UnsupportedProtocol { actual: AcpProtocolVersion },
    #[error("provider conversation is busy")]
    LocalBusy,
    #[error("provider does not advertise steering")]
    UnsupportedSteering,
    #[error("provider conversation operation is not active")]
    LocalNotFound,
    #[error("provider cancellation target is no longer active")]
    LocalCancelTargetMismatch,
    #[error("provider authentication is required (ACP code {code})")]
    AuthenticationRequired { code: i64 },
    #[error("provider session was not found (ACP code {code})")]
    ProviderSessionNotFound { code: i64 },
    #[error("provider resource was not found (ACP code {code})")]
    ResourceNotFound { code: i64 },
    #[error("provider ACP method is unsupported (ACP code {code})")]
    UnsupportedMethod { code: i64 },
    #[error("provider ACP parameters are invalid (ACP code {code})")]
    InvalidParams { code: i64 },
    #[error("provider ACP request was cancelled (ACP code {code})")]
    RequestCancelled { code: i64 },
    #[error("provider rejected the ACP operation (ACP code {code})")]
    ProviderRejected { code: i64 },
    #[error("provider operation response was unavailable")]
    TransportFailure,
    #[error("session history replay could not begin")]
    HistoryReplayUnavailable,
    #[error(
        "provider prompt output exceeded the retained output limit; cancellation was requested and settled"
    )]
    PromptOutputLimitExceeded,
    #[error("provider ACP frame exceeded the configured transport limit")]
    FrameLimitExceeded,
    #[error("provider ACP message could not be decoded or classified")]
    FrameDecodeFailure,
    #[error("unsupportedContent{{{content_type}}}")]
    UnsupportedContent { content_type: &'static str },
    #[error("agent ended the turn with an unrecognized stop reason{suffix}")]
    UnknownStopReason { suffix: String },
    #[error("provider ACP operation failed: {0}")]
    Operation(String),
}

impl ExternalProviderRuntimeError {
    pub fn unknown_stop_reason(value: &str) -> Self {
        let suffix = if !value.is_empty()
            && value.len() <= 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        {
            format!(" ({value})")
        } else {
            String::new()
        };
        Self::UnknownStopReason { suffix }
    }
}

pub(crate) fn sanitized_initialization_error(error: &agent_client_protocol::Error) -> String {
    sanitized_acp_error(error, "initialize", "initialize")
}

pub(crate) fn sanitized_acp_error(
    error: &agent_client_protocol::Error,
    method: &str,
    stage: &str,
) -> String {
    let error_data_bytes = error.data.as_ref().map_or(0, |data| data.to_string().len());
    format!(
        "ACP request failed (method={method}, stage={stage}, code={}, error_data_bytes={error_data_bytes})",
        i32::from(error.code),
    )
}

pub(crate) fn provider_frame_decode_error(
    error: agent_client_protocol::Error,
) -> ExternalProviderRuntimeError {
    if error.to_string().contains("max line length exceeded") {
        ExternalProviderRuntimeError::FrameLimitExceeded
    } else {
        ExternalProviderRuntimeError::FrameDecodeFailure
    }
}
