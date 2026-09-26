//! Host-owned ACP provider process admission and connection lifetime.

#[cfg(test)]
mod acp_scripted_fixture;
#[cfg(test)]
mod approval_dispatch_tests;
mod approval_presentation;
mod external_approval_dispatch;
mod external_permission_options;
mod provider_approval_dispatch;

use crate::provider_session_actor::{
    ProviderPromptDispatchObservation, ProviderSessionActivity, ProviderSessionCommand,
    ProviderSteeringOutcome, run_provider_session,
};
#[cfg(test)]
use agent_client_protocol::schema::v1::ToolKind;
use agent_client_protocol::schema::v1::{
    LoadSessionRequest, McpServer, McpServerHttp, NewSessionRequest, RequestPermissionRequest,
};
use agent_client_protocol::schema::{ProtocolVersion, v1::InitializeRequest};
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, ActiveSession, Agent, Client, ConnectionTo, Lines,
};
use collaboration_protocol::{CodexGeneration, OperationId, ProviderPromptStopReason, SessionRef};
use external_approval_dispatch::spawn_external_approval_dispatch;
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);
/// ACP JSON-RPC frames are capped at 64 MiB to allow large tool results while
/// keeping each provider connection's transport memory bounded.
const MAX_ACP_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_PROMPT_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderLaunch {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
}

impl ExternalProviderLaunch {
    fn sdk_config(&self) -> AcpAgentConfig {
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
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::MissingPromptContext => "missingPromptContext",
            Self::ApprovalBrokerUnavailable => "approvalBrokerUnavailable",
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderApprovalRefusalWarning {
    pub endpoint: String,
    pub provider_session_id: String,
    pub method: &'static str,
    pub reason_code: ExternalProviderApprovalRefusalReason,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderToolCall {
    pub name: Option<String>,
    pub title: String,
    pub kind: ToolKind,
    pub status: agent_client_protocol::schema::v1::ToolCallStatus,
    pub outcome: ExternalProviderToolOutcome,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalProviderToolOutcome {
    Success,
    Error,
    Rejected,
    PermissionDenied,
    Unknown,
}

#[cfg(test)]
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

#[cfg(test)]
fn has_completed_router_endpoints_call(tool_calls: &[ExternalProviderToolCall]) -> bool {
    tool_calls.iter().any(|tool_call| {
        (tool_call.name.as_deref() == Some("router-collaboration-endpoints_list")
            || tool_call.title == "router-collaboration: endpoints_list")
            && tool_call.kind == ToolKind::Other
            && tool_call.status == agent_client_protocol::schema::v1::ToolCallStatus::Completed
            && tool_call.outcome == ExternalProviderToolOutcome::Success
    })
}

#[cfg(test)]
fn accepts_native_router_result(
    tool_calls: &[ExternalProviderToolCall],
    response: &str,
    expected_service_id: &collaboration_protocol::UuidIdentity,
) -> bool {
    let output = classify_provider_output(response, expected_service_id);
    has_completed_router_endpoints_call(tool_calls)
        && output.returned_uuid_count == 1
        && output.sole_uuid_matches_expected
}

#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
struct ProviderOutputClassification {
    output_empty: bool,
    exact_identity_equal: bool,
    expected_identity_token_present: bool,
    returned_uuid_count: usize,
    sole_uuid_matches_expected: bool,
    output_byte_count: usize,
}

#[cfg(test)]
fn classify_provider_output(
    output: &str,
    expected_service_id: &collaboration_protocol::UuidIdentity,
) -> ProviderOutputClassification {
    fn is_uuid_shape(candidate: &[u8]) -> bool {
        candidate.len() == 36
            && candidate.iter().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    *byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
    }

    fn continues_token(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
    }

    let expected = String::from(expected_service_id.clone());
    let bytes = output.as_bytes();
    let mut returned_uuids = Vec::new();
    let mut start = 0_usize;
    while start.saturating_add(36) <= bytes.len() {
        let end = start + 36;
        let bounded_before = start == 0 || !continues_token(bytes[start - 1]);
        let bounded_after = end == bytes.len() || !continues_token(bytes[end]);
        if bounded_before
            && bounded_after
            && is_uuid_shape(&bytes[start..end])
            && let Ok(candidate) = std::str::from_utf8(&bytes[start..end])
            && let Ok(identity) =
                collaboration_protocol::UuidIdentity::try_from(candidate.to_owned())
        {
            returned_uuids.push(identity);
            start = end;
            continue;
        }
        start += 1;
    }
    let expected_identity_token_present = returned_uuids
        .iter()
        .any(|identity| identity == expected_service_id);
    ProviderOutputClassification {
        output_empty: output.is_empty(),
        exact_identity_equal: output.trim() == expected,
        expected_identity_token_present,
        returned_uuid_count: returned_uuids.len(),
        sole_uuid_matches_expected: returned_uuids.len() == 1 && expected_identity_token_present,
        output_byte_count: output.len(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderCreatedSession {
    pub provider_session_id: String,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalProviderPermissionOutcome {
    Cancelled,
    Selected,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalProviderPermissionObservation {
    pub method: &'static str,
    pub request_count: u64,
    pub last_outcome: Option<ExternalProviderPermissionOutcome>,
    pub execute_tool_call_count: u64,
}

#[derive(Clone, Debug)]
pub struct ExternalProviderApprovalContext {
    pub requester: SessionRef,
    pub approver: SessionRef,
    pub target: SessionRef,
    pub operation_id: OperationId,
    pub binding_generation: CodexGeneration,
    pub binding_retirement: CancellationToken,
}

struct ApprovalContextGuard {
    contexts: Arc<std::sync::Mutex<HashMap<String, ExternalProviderApprovalContext>>>,
    provider_session_id: String,
    operation_id: OperationId,
}

impl Drop for ApprovalContextGuard {
    fn drop(&mut self) {
        if let Ok(mut contexts) = self.contexts.lock()
            && contexts
                .get(&self.provider_session_id)
                .is_some_and(|context| context.operation_id == self.operation_id)
        {
            contexts.remove(&self.provider_session_id);
        }
    }
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
    UnsupportedProtocol { actual: ProtocolVersion },
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
    #[error("provider rejected the ACP operation (ACP code {code})")]
    ProviderRejected { code: i64 },
    #[error("provider operation response was unavailable")]
    TransportFailure,
    #[error(
        "provider prompt output exceeded the retained output limit; cancellation was requested and settled"
    )]
    PromptOutputLimitExceeded,
    #[error("provider ACP frame exceeded the configured transport limit")]
    FrameLimitExceeded,
    #[error("provider ACP message could not be decoded or classified")]
    FrameDecodeFailure,
    #[error("provider ACP operation failed: {0}")]
    Operation(String),
}

pub(crate) fn acp_operation_error(
    error: agent_client_protocol::Error,
) -> ExternalProviderRuntimeError {
    use agent_client_protocol::schema::v1::ErrorCode;
    let code = i64::from(i32::from(error.code));
    match error.code {
        ErrorCode::AuthRequired => ExternalProviderRuntimeError::AuthenticationRequired { code },
        ErrorCode::ResourceNotFound => {
            ExternalProviderRuntimeError::ProviderSessionNotFound { code }
        }
        _ => ExternalProviderRuntimeError::ProviderRejected { code },
    }
}

pub(crate) fn sanitized_initialization_error(error: &agent_client_protocol::Error) -> String {
    sanitized_acp_error(error, "initialize", "initialize")
}

fn sanitized_acp_error(error: &agent_client_protocol::Error, method: &str, stage: &str) -> String {
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

enum ProviderCommand {
    Create {
        cwd: PathBuf,
        reply: tokio::sync::oneshot::Sender<
            Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError>,
        >,
    },
    Load {
        provider_session_id: String,
        cwd: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    Prompt {
        provider_session_id: String,
        operation_id: Option<OperationId>,
        prompt: String,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
        reply: tokio::sync::oneshot::Sender<
            Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError>,
        >,
    },
    Cancel {
        provider_session_id: String,
        expected_operation_id: Option<OperationId>,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    Steer {
        provider_session_id: String,
        prompt: String,
        reply: tokio::sync::oneshot::Sender<
            Result<ProviderSteeringOutcome, ExternalProviderRuntimeError>,
        >,
    },
    InspectSession {
        provider_session_id: String,
        reply: tokio::sync::oneshot::Sender<ProviderSessionActivity>,
    },
    WaitSessionIdle {
        provider_session_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
}

enum PendingSessionAdmission {
    Create {
        result: Result<ProviderSessionRegistration, ExternalProviderRuntimeError>,
        reply: tokio::sync::oneshot::Sender<
            Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError>,
        >,
    },
    Load {
        provider_session_id: String,
        result: Box<Result<ActiveSession<'static, Agent>, ExternalProviderRuntimeError>>,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
}

struct ProviderSessionRegistration {
    provider_session_id: String,
    commands: tokio::sync::mpsc::Sender<ProviderSessionCommand>,
}

#[derive(Debug, Default)]
pub(crate) struct ProviderFrameObservation {
    limit_exceeded: AtomicBool,
    limit_notification: tokio::sync::Notify,
}

impl ProviderFrameObservation {
    fn record_limit_exceeded(&self) {
        self.limit_exceeded.store(true, Ordering::Relaxed);
        self.limit_notification.notify_one();
    }

    pub(crate) fn limit_was_exceeded(&self) -> bool {
        self.limit_exceeded.load(Ordering::Relaxed)
    }

    pub(crate) async fn wait_for_limit_exceeded(&self) -> bool {
        tokio::time::timeout(
            Duration::from_millis(100),
            self.limit_notification.notified(),
        )
        .await
        .is_ok()
    }
}

/// Owns the provider process and ACP connection independently of caller tasks.
pub struct ExternalProviderRuntime {
    admission: ExternalProviderAdmission,
    shutdown: CancellationToken,
    retirement: CancellationToken,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    shutdown_failed: Arc<std::sync::atomic::AtomicBool>,
    frame_observation: Arc<ProviderFrameObservation>,
    commands: tokio::sync::mpsc::Sender<ProviderCommand>,
    #[cfg(test)]
    permission_request_count: Arc<AtomicU64>,
    #[cfg(test)]
    permission_outcome: Arc<std::sync::atomic::AtomicU8>,
    approval_broker: Arc<
        tokio::sync::RwLock<Option<std::sync::Weak<collaboration_service::ServiceApprovalBroker>>>,
    >,
    approval_contexts: Arc<std::sync::Mutex<HashMap<String, ExternalProviderApprovalContext>>>,
    permission_refusal_reasons:
        Arc<std::sync::Mutex<HashMap<OperationId, ExternalProviderApprovalRefusalReason>>>,
    endpoint_id: Arc<tokio::sync::RwLock<Option<String>>>,
    #[cfg(test)]
    approval_refusal_warnings: Arc<std::sync::Mutex<Vec<ExternalProviderApprovalRefusalWarning>>>,
    #[cfg(test)]
    test_tool_calls: Arc<std::sync::Mutex<Vec<ExternalProviderToolCall>>>,
}

impl std::fmt::Debug for ExternalProviderRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalProviderRuntime")
            .field("admission", &self.admission)
            .finish_non_exhaustive()
    }
}

impl ExternalProviderRuntime {
    pub async fn initialize(
        launch: ExternalProviderLaunch,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(launch, INITIALIZE_TIMEOUT, Vec::new()).await
    }

    pub async fn initialize_with_mcp_http(
        launch: ExternalProviderLaunch,
        server_name: impl Into<String>,
        server_url: impl Into<String>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            INITIALIZE_TIMEOUT,
            vec![McpServer::Http(McpServerHttp::new(server_name, server_url))],
        )
        .await
    }

    #[cfg(test)]
    async fn initialize_with_timeout(
        launch: ExternalProviderLaunch,
        initialize_timeout: Duration,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(launch, initialize_timeout, Vec::new()).await
    }

    async fn initialize_with_timeout_and_mcp_servers(
        launch: ExternalProviderLaunch,
        initialize_timeout: Duration,
        configured_mcp_servers: Vec<McpServer>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        let agent = AcpAgent::new(launch.sdk_config());
        let (stdin, stdout, mut stderr, mut child) = agent
            .spawn_process()
            .map_err(|error| ExternalProviderRuntimeError::Launch(error.to_string()))?;
        let shutdown = CancellationToken::new();
        let retirement = CancellationToken::new();
        let frame_observation = Arc::new(ProviderFrameObservation::default());
        let task_frame_observation = Arc::clone(&frame_observation);
        let shutdown_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_shutdown_failed = Arc::clone(&shutdown_failed);
        let task_shutdown = shutdown.clone();
        let connection_retirement = retirement.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(32);
        #[cfg(test)]
        let permission_request_count = Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let callback_permission_request_count = Arc::clone(&permission_request_count);
        #[cfg(test)]
        let permission_outcome = Arc::new(std::sync::atomic::AtomicU8::new(0));
        #[cfg(test)]
        let callback_permission_outcome = Arc::clone(&permission_outcome);
        let approval_broker = Arc::new(tokio::sync::RwLock::new(
            None::<std::sync::Weak<collaboration_service::ServiceApprovalBroker>>,
        ));
        let callback_approval_broker = Arc::clone(&approval_broker);
        let approval_contexts = Arc::new(std::sync::Mutex::new(HashMap::<
            String,
            ExternalProviderApprovalContext,
        >::new()));
        let callback_approval_contexts = Arc::clone(&approval_contexts);
        let permission_refusal_reasons = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let callback_permission_refusal_reasons = Arc::clone(&permission_refusal_reasons);
        let endpoint_id = Arc::new(tokio::sync::RwLock::new(None::<String>));
        let callback_endpoint_id = Arc::clone(&endpoint_id);
        #[cfg(test)]
        let approval_refusal_warnings = Arc::new(std::sync::Mutex::new(Vec::<
            ExternalProviderApprovalRefusalWarning,
        >::new()));
        #[cfg(test)]
        let callback_approval_refusal_warnings = Arc::clone(&approval_refusal_warnings);
        #[cfg(test)]
        let test_tool_calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        #[cfg(test)]
        let session_test_tool_calls = Arc::clone(&test_tool_calls);
        let task = tokio::spawn(async move {
            use futures_util::StreamExt as _;
            use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
            use tokio_util::compat::{
                FuturesAsyncReadCompatExt as _, FuturesAsyncWriteCompatExt as _,
            };

            let stderr_task = tokio::spawn(async move {
                use futures_util::io::AsyncReadExt as _;
                let mut buffer = [0_u8; 8 * 1024];
                while stderr.read(&mut buffer).await.unwrap_or(0) != 0 {}
            });
            let outgoing = futures_util::SinkExt::<String>::sink_map_err(
                FramedWrite::new(stdin.compat_write(), LinesCodec::new()),
                std::io::Error::other,
            );
            let incoming_frame_observation = Arc::clone(&task_frame_observation);
            let incoming = FramedRead::new(
                stdout.compat(),
                LinesCodec::new_with_max_length(MAX_ACP_FRAME_BYTES),
            )
            .map(move |line| {
                line.map_err(|error| {
                    if matches!(
                        error,
                        tokio_util::codec::LinesCodecError::MaxLineLengthExceeded
                    ) {
                        incoming_frame_observation.record_limit_exceeded();
                    }
                    std::io::Error::other(error)
                })
            });
            let connection = Client.builder().name("codex-router-host")
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder, connection| {
                        #[cfg(test)]
                        callback_permission_request_count.fetch_add(1, Ordering::Relaxed);
                        let context = callback_approval_contexts.lock().ok().and_then(|contexts| {
                            contexts.get(request.session_id.0.as_ref()).cloned()
                        });
                        let broker = callback_approval_broker
                            .read()
                            .await
                            .as_ref()
                            .and_then(std::sync::Weak::upgrade);
                        if context.is_none() || broker.is_none() {
                            let reason = if context.is_none() {
                                ExternalProviderApprovalRefusalReason::MissingPromptContext
                            } else {
                                ExternalProviderApprovalRefusalReason::ApprovalBrokerUnavailable
                            };
                            let endpoint = callback_endpoint_id
                                .read()
                                .await
                                .clone()
                                .unwrap_or_else(|| "unknown".to_owned());
                            let provider_session_id = request.session_id.0.to_string();
                            tracing::warn!(
                                endpoint = %endpoint,
                                provider_session_id = %provider_session_id,
                                method = "session/request_permission",
                                reason_code = reason.code(),
                                "provider permission request refused before approval broker",
                            );
                            #[cfg(test)]
                            if let Ok(mut warnings) = callback_approval_refusal_warnings.lock() {
                                warnings.push(ExternalProviderApprovalRefusalWarning {
                                    endpoint,
                                    provider_session_id,
                                    method: "session/request_permission",
                                    reason_code: reason,
                                });
                            }
                            if let Some(context) = &context
                                && let Ok(mut refusals) =
                                    callback_permission_refusal_reasons.lock()
                            {
                                refusals.insert(context.operation_id.clone(), reason);
                            }
                        }
                        spawn_external_approval_dispatch(
                            request,
                            responder,
                            connection,
                            broker,
                            context,
                            #[cfg(test)]
                            Arc::clone(&callback_permission_outcome),
                        )
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(
                Lines::new(outgoing, incoming),
                async move |connection| {
                    let initialized = tokio::select! {
                        biased;
                        () = task_shutdown.cancelled() => {
                            return Ok(());
                        }
                        response = connection
                            .send_request(InitializeRequest::new(ProtocolVersion::V1))
                            .block_task() => response,
                    };
                    let admission = match initialized {
                        Ok(response) if response.protocol_version == ProtocolVersion::V1 => {
                            Ok(ExternalProviderAdmission {
                                runtime_name: response
                                    .agent_info
                                    .as_ref()
                                    .map(|info| info.name.clone()),
                                runtime_version: response
                                    .agent_info
                                    .as_ref()
                                    .map(|info| info.version.clone()),
                                supports_load: response.agent_capabilities.load_session,
                                supports_mcp_http: response
                                    .agent_capabilities
                                    .mcp_capabilities
                                    .http,
                                supports_steering: response.meta.as_ref()
                                    .and_then(|meta| meta.get("steering"))
                                    .and_then(|steering| steering.get("supported"))
                                    .and_then(serde_json::Value::as_bool)
                                    == Some(true),
                            })
                        }
                        Ok(response) => Err(ExternalProviderRuntimeError::UnsupportedProtocol {
                            actual: response.protocol_version,
                        }),
                        Err(error) => {
                            Err(ExternalProviderRuntimeError::Initialize(
                                sanitized_initialization_error(&error),
                            ))
                        }
                    };
                    let admitted = admission.is_ok();
                    let session_mcp_servers = if matches!(
                        &admission,
                        Ok(admission) if admission.supports_mcp_http
                    ) {
                        configured_mcp_servers
                    } else {
                        Vec::new()
                    };
                    let _result = ready_tx.send(admission);
                    if admitted {
                        let mut sessions = HashMap::<
                            String,
                            tokio::sync::mpsc::Sender<ProviderSessionCommand>,
                        >::new();
                        let mut session_tasks = tokio::task::JoinSet::new();
                        let mut admission_tasks = tokio::task::JoinSet::new();
                        let (admission_tx, mut admission_rx) = tokio::sync::mpsc::channel(32);
                        let mut pending_loads = std::collections::HashSet::<String>::new();
                        loop {
                            tokio::select! {
                                () = task_shutdown.cancelled() => break,
                                completion = admission_rx.recv() => {
                                    let Some(completion) = completion else { continue; };
                                    match completion {
                                        PendingSessionAdmission::Create { result, reply } => {
                                            let result = result.and_then(|registration| {
                                                register_provider_session(registration, &mut sessions)
                                            });
                                            let _result = reply.send(result);
                                        }
                                        PendingSessionAdmission::Load { provider_session_id, result, reply } => {
                                            pending_loads.remove(&provider_session_id);
                                            let result = (*result).and_then(|mut session| {
                                                discard_queued_session_updates(&mut session)?;
                                                register_static_provider_session(
                                                    session,
                                                    &mut sessions,
                                                    &mut session_tasks,
                                                    task_shutdown.clone(),
                                                    Arc::clone(&task_frame_observation),
                                                    #[cfg(test)] Arc::clone(&session_test_tool_calls),
                                                )
                                            }).map(|_| ());
                                            let _result = reply.send(result);
                                        }
                                    }
                                }
                                completion = admission_tasks.join_next(), if !admission_tasks.is_empty() => {
                                    if matches!(completion, Some(Err(_))) {
                                        task_shutdown_failed.store(true, Ordering::Relaxed);
                                        break;
                                    }
                                }
                                command = command_rx.recv() => {
                                    let Some(command) = command else { break; };
                                    match command {
                                        ProviderCommand::Create { cwd, reply } => {
                                            let request = NewSessionRequest::new(&cwd)
                                                .mcp_servers(session_mcp_servers.clone());
                                            let pending_connection = connection.clone();
                                            let pending_shutdown = task_shutdown.clone();
                                            let pending_admission_tx = admission_tx.clone();
                                            let pending_frame_observation = Arc::clone(&task_frame_observation);
                                            #[cfg(test)]
                                            let pending_test_tool_calls = Arc::clone(&session_test_tool_calls);
                                            admission_tasks.spawn(async move {
                                                run_create_admission(
                                                    pending_connection,
                                                    request,
                                                    pending_shutdown,
                                                    pending_admission_tx,
                                                    reply,
                                                    pending_frame_observation,
                                                    #[cfg(test)] pending_test_tool_calls,
                                                )
                                                .await;
                                            });
                                        }
                                        ProviderCommand::Load { provider_session_id, cwd, reply } => {
                                            if sessions.contains_key(&provider_session_id) || !pending_loads.insert(provider_session_id.clone()) {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalBusy));
                                            } else {
                                                let request = LoadSessionRequest::new(
                                                    provider_session_id.clone(),
                                                    &cwd,
                                                )
                                                .mcp_servers(session_mcp_servers.clone());
                                                let pending_connection = connection.clone();
                                                let pending_shutdown = task_shutdown.clone();
                                                let pending_admission_tx = admission_tx.clone();
                                                admission_tasks.spawn(async move {
                                                    let result = tokio::select! {
                                                        () = pending_shutdown.cancelled() => Err(ExternalProviderRuntimeError::TransportFailure),
                                                        result = pending_connection
                                                        .load_session_from(request)
                                                        .block_task()
                                                        .start_session() => result.map(|restored| restored.into_session()).map_err(acp_operation_error),
                                                    };
                                                    publish_pending_session_admission(
                                                        &pending_admission_tx,
                                                        &pending_shutdown,
                                                        PendingSessionAdmission::Load {
                                                            provider_session_id,
                                                            result: Box::new(result),
                                                            reply,
                                                        },
                                                    ).await;
                                                });
                                            }
                                        }
                                        ProviderCommand::Prompt { provider_session_id, operation_id, prompt, dispatch, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                if let Some(dispatch) = dispatch {
                                                    let _result = dispatch.send(ProviderPromptDispatchObservation::NotSubmitted);
                                                }
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                                                continue;
                                            };
                                            if let Err(error) = session.send(ProviderSessionCommand::Prompt { operation_id, prompt, dispatch, reply }).await
                                                && let ProviderSessionCommand::Prompt { dispatch: Some(dispatch), .. } = error.0
                                            {
                                                let _result = dispatch.send(ProviderPromptDispatchObservation::NotSubmitted);
                                            }
                                        }
                                        ProviderCommand::Cancel { provider_session_id, expected_operation_id, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                                                continue;
                                            };
                                            let _result = session.send(ProviderSessionCommand::Cancel { expected_operation_id, reply }).await;
                                        }
                                        ProviderCommand::Steer { provider_session_id, prompt, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                                                continue;
                                            };
                                            let _result = session.send(ProviderSessionCommand::Steer { prompt, reply }).await;
                                        }
                                        ProviderCommand::InspectSession { provider_session_id, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                let _result = reply.send(ProviderSessionActivity::NotLoaded);
                                                continue;
                                            };
                                            let _result = session.send(ProviderSessionCommand::Inspect { reply }).await;
                                        }
                                        ProviderCommand::WaitSessionIdle { provider_session_id, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                                                continue;
                                            };
                                            let _result = session.send(ProviderSessionCommand::WaitIdle { reply }).await;
                                        }
                                    }
                                }
                            }
                        }
                        admission_rx.close();
                        while let Ok(completion) = admission_rx.try_recv() {
                            fail_pending_session_admission(completion);
                        }
                        task_shutdown.cancel();
                        while let Some(result) = admission_tasks.join_next().await {
                            if result.is_err() {
                                task_shutdown_failed.store(true, Ordering::Relaxed);
                            }
                        }
                        while let Some(result) = session_tasks.join_next().await {
                            if result.is_err() {
                                task_shutdown_failed.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                    Ok(())
                },
            );
            let child_exited = tokio::select! {
                _result = connection => false,
                _status = child.status() => true,
            };
            connection_retirement.cancel();
            #[cfg(unix)]
            if !child_exited
                && let Some(process_id) = rustix::process::Pid::from_raw(child.id().cast_signed())
            {
                let _result =
                    rustix::process::kill_process_group(process_id, rustix::process::Signal::KILL);
            }
            if !child_exited {
                let _result = child.kill();
                let _result = child.status().await;
            }
            let _result = stderr_task.await;
        });

        let admission = match tokio::time::timeout(initialize_timeout, ready_rx).await {
            Ok(Ok(Ok(admission))) => admission,
            Ok(Ok(Err(error))) => {
                shutdown.cancel();
                let _result = task.await;
                return Err(error);
            }
            Ok(Err(_)) => {
                shutdown.cancel();
                let _result = task.await;
                return Err(ExternalProviderRuntimeError::Initialize(
                    "provider connection closed before initialization".to_owned(),
                ));
            }
            Err(_) => {
                shutdown.cancel();
                let _result = task.await;
                return Err(ExternalProviderRuntimeError::InitializeTimeout);
            }
        };
        Ok(Self {
            admission,
            shutdown,
            retirement,
            task: tokio::sync::Mutex::new(Some(task)),
            shutdown_failed,
            frame_observation,
            commands: command_tx,
            #[cfg(test)]
            permission_request_count,
            #[cfg(test)]
            permission_outcome,
            approval_broker,
            approval_contexts,
            permission_refusal_reasons,
            endpoint_id,
            #[cfg(test)]
            approval_refusal_warnings,
            #[cfg(test)]
            test_tool_calls,
        })
    }

    #[must_use]
    pub fn admission(&self) -> &ExternalProviderAdmission {
        &self.admission
    }

    #[must_use]
    #[cfg(test)]
    pub fn permission_observation(&self) -> ExternalProviderPermissionObservation {
        let request_count = self.permission_request_count.load(Ordering::Relaxed);
        ExternalProviderPermissionObservation {
            method: "session/request_permission",
            request_count,
            last_outcome: match self.permission_outcome.load(Ordering::Relaxed) {
                1 => Some(ExternalProviderPermissionOutcome::Cancelled),
                2 => Some(ExternalProviderPermissionOutcome::Selected),
                _ => None,
            },
            execute_tool_call_count: 0,
        }
    }

    #[cfg(test)]
    fn take_test_tool_calls(&self) -> Vec<ExternalProviderToolCall> {
        std::mem::take(&mut *self.test_tool_calls.lock().expect("test tool calls"))
    }

    #[must_use]
    pub fn retirement(&self) -> CancellationToken {
        self.retirement.clone()
    }

    pub async fn install_approval_broker(
        &self,
        broker: Arc<collaboration_service::ServiceApprovalBroker>,
    ) {
        *self.approval_broker.write().await = Some(Arc::downgrade(&broker));
    }

    pub async fn set_endpoint_id(&self, endpoint_id: String) {
        *self.endpoint_id.write().await = Some(endpoint_id);
    }

    #[cfg(test)]
    pub fn approval_refusal_warnings(&self) -> Vec<ExternalProviderApprovalRefusalWarning> {
        self.approval_refusal_warnings
            .lock()
            .map(|warnings| warnings.clone())
            .unwrap_or_default()
    }

    pub async fn prompt_with_approval_context(
        &self,
        provider_session_id: String,
        prompt: String,
        context: ExternalProviderApprovalContext,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_with_approval_dispatch(provider_session_id, prompt, context, None)
            .await
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
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Create { cwd, reply })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
    }

    pub async fn load_session(
        &self,
        provider_session_id: String,
        cwd: PathBuf,
    ) -> Result<(), ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Load {
                provider_session_id,
                cwd,
                reply,
            })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
    }

    pub async fn prompt(
        &self,
        provider_session_id: String,
        prompt: String,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_for_operation(provider_session_id, None, prompt, None)
            .await
    }

    pub async fn steer_session(
        &self,
        provider_session_id: String,
        prompt: String,
    ) -> Result<ProviderSteeringOutcome, ExternalProviderRuntimeError> {
        if !self.admission.supports_steering {
            return Err(ExternalProviderRuntimeError::UnsupportedSteering);
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Steer {
                provider_session_id,
                prompt,
                reply,
            })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
    }

    pub async fn session_activity(
        &self,
        provider_session_id: String,
    ) -> Result<ProviderSessionActivity, ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::InspectSession {
                provider_session_id,
                reply,
            })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)
    }

    pub async fn wait_session_idle(
        &self,
        provider_session_id: String,
    ) -> Result<(), ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::WaitSessionIdle {
                provider_session_id,
                reply,
            })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
    }

    async fn prompt_for_operation(
        &self,
        provider_session_id: String,
        operation_id: Option<OperationId>,
        prompt: String,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
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

    #[cfg(test)]
    pub async fn cancel_active_prompt(
        &self,
        provider_session_id: String,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.cancel_prompt(provider_session_id, None).await
    }

    pub async fn cancel_prompt_operation(
        &self,
        provider_session_id: String,
        expected_operation_id: OperationId,
    ) -> Result<(), ExternalProviderRuntimeError> {
        self.cancel_prompt(provider_session_id, Some(expected_operation_id))
            .await
    }

    async fn cancel_prompt(
        &self,
        provider_session_id: String,
        expected_operation_id: Option<OperationId>,
    ) -> Result<(), ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Cancel {
                provider_session_id,
                expected_operation_id,
                reply,
            })
            .await
            .map_err(|_| {
                ExternalProviderRuntimeError::Operation("provider runtime closed".to_owned())
            })?;
        result.await.map_err(|_| {
            ExternalProviderRuntimeError::Operation("provider runtime closed".to_owned())
        })?
    }

    pub async fn shutdown(&self) {
        self.retirement.cancel();
        self.shutdown.cancel();
        if let Some(task) = self.task.lock().await.take()
            && task.await.is_err()
        {
            self.shutdown_failed.store(true, Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn shutdown_failed(&self) -> bool {
        self.shutdown_failed.load(Ordering::Relaxed)
    }
}

fn register_provider_session(
    registration: ProviderSessionRegistration,
    sessions: &mut HashMap<String, tokio::sync::mpsc::Sender<ProviderSessionCommand>>,
) -> Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError> {
    if sessions.contains_key(&registration.provider_session_id) {
        return Err(ExternalProviderRuntimeError::Operation(
            "provider returned a duplicate conversation identity".to_owned(),
        ));
    }
    sessions.insert(
        registration.provider_session_id.clone(),
        registration.commands,
    );
    Ok(ExternalProviderCreatedSession {
        provider_session_id: registration.provider_session_id,
    })
}

fn register_static_provider_session(
    session: ActiveSession<'static, Agent>,
    sessions: &mut HashMap<String, tokio::sync::mpsc::Sender<ProviderSessionCommand>>,
    session_tasks: &mut tokio::task::JoinSet<()>,
    shutdown: CancellationToken,
    frame_observation: Arc<ProviderFrameObservation>,
    #[cfg(test)] test_tool_calls: Arc<std::sync::Mutex<Vec<ExternalProviderToolCall>>>,
) -> Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError> {
    let provider_session_id = session.session_id().to_string();
    if sessions.contains_key(&provider_session_id) {
        return Err(ExternalProviderRuntimeError::Operation(
            "provider returned a duplicate conversation identity".to_owned(),
        ));
    }
    let (commands, command_rx) = tokio::sync::mpsc::channel(16);
    sessions.insert(provider_session_id.clone(), commands);
    session_tasks.spawn(run_provider_session(
        session,
        command_rx,
        shutdown,
        frame_observation,
        #[cfg(test)]
        test_tool_calls,
    ));
    Ok(ExternalProviderCreatedSession {
        provider_session_id,
    })
}

async fn run_create_admission(
    connection: ConnectionTo<Agent>,
    request: NewSessionRequest,
    shutdown: CancellationToken,
    admission_tx: tokio::sync::mpsc::Sender<PendingSessionAdmission>,
    reply: tokio::sync::oneshot::Sender<
        Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError>,
    >,
    frame_observation: Arc<ProviderFrameObservation>,
    #[cfg(test)] test_tool_calls: Arc<std::sync::Mutex<Vec<ExternalProviderToolCall>>>,
) {
    let (registration_tx, registration_rx) = tokio::sync::oneshot::channel();
    let session_shutdown = shutdown.clone();
    let session_frame_observation = Arc::clone(&frame_observation);
    let session_future = connection
        .build_session_from(request)
        .block_task()
        .run_until(async move |session| {
            let provider_session_id = session.session_id().to_string();
            let (commands, command_rx) = tokio::sync::mpsc::channel(16);
            registration_tx
                .send(ProviderSessionRegistration {
                    provider_session_id,
                    commands,
                })
                .map_err(|_| agent_client_protocol::Error::internal_error())?;
            run_provider_session(
                session,
                command_rx,
                session_shutdown,
                session_frame_observation,
                #[cfg(test)]
                test_tool_calls,
            )
            .await;
            Ok(())
        });
    tokio::pin!(session_future);
    let result = tokio::select! {
        biased;
        () = shutdown.cancelled() => Err(ExternalProviderRuntimeError::TransportFailure),
        registration = registration_rx => registration
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure),
        result = &mut session_future => Err(result
            .err()
            .map_or(ExternalProviderRuntimeError::TransportFailure, acp_operation_error)),
    };
    let registered = result.is_ok();
    publish_pending_session_admission(
        &admission_tx,
        &shutdown,
        PendingSessionAdmission::Create { result, reply },
    )
    .await;
    if registered {
        let _result = tokio::select! {
            () = shutdown.cancelled() => Ok(()),
            result = &mut session_future => result,
        };
    }
}

async fn publish_pending_session_admission(
    admission_tx: &tokio::sync::mpsc::Sender<PendingSessionAdmission>,
    shutdown: &CancellationToken,
    completion: PendingSessionAdmission,
) {
    tokio::select! {
        biased;
        () = shutdown.cancelled() => fail_pending_session_admission(completion),
        permit = admission_tx.reserve() => match permit {
            Ok(permit) => permit.send(completion),
            Err(_) => fail_pending_session_admission(completion),
        },
    }
}

fn fail_pending_session_admission(completion: PendingSessionAdmission) {
    match completion {
        PendingSessionAdmission::Create { reply, .. } => {
            let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
        }
        PendingSessionAdmission::Load { reply, .. } => {
            let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
        }
    }
}

fn discard_queued_session_updates(
    session: &mut ActiveSession<'static, Agent>,
) -> Result<(), ExternalProviderRuntimeError> {
    use futures_util::FutureExt as _;

    loop {
        match session.read_update().now_or_never() {
            Some(Ok(_update)) => continue,
            Some(Err(error)) => {
                return Err(provider_frame_decode_error(error));
            }
            None => return Ok(()),
        }
    }
}

impl Drop for ExternalProviderRuntime {
    fn drop(&mut self) {
        self.retirement.cancel();
        self.shutdown.cancel();
    }
}

#[cfg(test)]
mod live_tests;
#[cfg(test)]
#[path = "external_provider_runtime/robustness_tests.rs"]
mod robustness_tests;
#[cfg(test)]
mod tests;
