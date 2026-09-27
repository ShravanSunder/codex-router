//! ACP provider process admission and connection lifetime.

mod approval_turn_cancellation;
mod external_approval_dispatch;
mod provider_acp_error_mapping;
mod provider_approval_dispatch;
mod provider_client_operations;
mod provider_frame_observation;
mod provider_initialize_request;
mod provider_prompt_dispatch;
mod provider_request_fallback;
mod provider_session_admission;

use crate::ProviderCapabilityReport;
use crate::provider_prompt_content::ProviderPromptContent;
use crate::provider_session_actor::{
    ProviderPromptDispatchObservation, ProviderSessionActivity, ProviderSessionCommand,
    ProviderSteeringOutcome, run_provider_session,
};
use crate::{InteractionPort, SessionEventSink};
use agent_client_protocol::schema::ProtocolVersion;
#[cfg(any(test, feature = "test-observation"))]
use agent_client_protocol::schema::v1::ToolKind;
use agent_client_protocol::schema::v1::{
    LoadSessionRequest, McpServer, McpServerHttp, NewSessionRequest, NewSessionResponse,
    RequestPermissionRequest,
};
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, ActiveSession, Agent, Client, ConnectionTo, Lines,
};
pub(crate) use approval_turn_cancellation::ProviderTurnCancellation;
use approval_turn_cancellation::{ActiveApprovalContext, active_turn_cancellation};
use external_approval_dispatch::{PermissionDispatchState, spawn_external_approval_dispatch};
use provider_acp_error_mapping::acp_load_session_error;
pub use provider_acp_error_mapping::acp_operation_error;
pub(crate) use provider_frame_observation::ProviderFrameObservation;
use provider_initialize_request::initialize_provider_connection;
use provider_request_fallback::{
    ProviderKnownSessions, ProviderRequestFallback, ProviderRequestSessionGuard,
};
use provider_session_admission::*;
use session_event_model::StopReason as ProviderPromptStopReason;
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(any(test, feature = "test-observation"))]
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

struct ApprovalContextGuard<P: InteractionPort> {
    contexts: Arc<std::sync::Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
    provider_session_id: String,
    operation_id: P::OperationId,
}

impl<P: InteractionPort> Drop for ApprovalContextGuard<P> {
    fn drop(&mut self) {
        if let Ok(mut contexts) = self.contexts.lock()
            && contexts
                .get(&self.provider_session_id)
                .is_some_and(|context| P::operation_id(&context.approval) == self.operation_id)
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

pub fn sanitized_initialization_error(error: &agent_client_protocol::Error) -> String {
    sanitized_acp_error(error, "initialize", "initialize")
}

pub fn sanitized_acp_error(
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

enum ProviderCommand<P: InteractionPort> {
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
        operation_id: Option<P::OperationId>,
        prompt: ProviderPromptContent,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
        reply: tokio::sync::oneshot::Sender<
            Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError>,
        >,
    },
    Cancel {
        provider_session_id: String,
        expected_operation_id: Option<P::OperationId>,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    Steer {
        provider_session_id: String,
        prompt: String,
        reply: tokio::sync::oneshot::Sender<
            Result<ProviderSteeringOutcome<P::OperationId>, ExternalProviderRuntimeError>,
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

enum PendingSessionAdmission<P: InteractionPort> {
    Create {
        result: Box<Result<ProviderSessionRegistration<P>, ExternalProviderRuntimeError>>,
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

struct ProviderSessionRegistration<P: InteractionPort> {
    provider_session_id: String,
    commands: tokio::sync::mpsc::Sender<ProviderSessionCommand<P>>,
    response: NewSessionResponse,
}

/// Owns the provider process and ACP connection independently of caller tasks.
pub struct AgentSessionClient<P: InteractionPort> {
    admission: ExternalProviderAdmission,
    base_capabilities: ProviderCapabilityReport,
    session_capabilities: Arc<tokio::sync::RwLock<HashMap<String, ProviderCapabilityReport>>>,
    shutdown: CancellationToken,
    retirement: CancellationToken,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    shutdown_failed: Arc<std::sync::atomic::AtomicBool>,
    frame_observation: Arc<ProviderFrameObservation>,
    commands: tokio::sync::mpsc::Sender<ProviderCommand<P>>,
    #[cfg(any(test, feature = "test-observation"))]
    permission_request_count: Arc<AtomicU64>,
    #[cfg(any(test, feature = "test-observation"))]
    permission_outcome: Arc<std::sync::atomic::AtomicU8>,
    interaction_port: Arc<P>,
    _event_sink: Arc<dyn SessionEventSink>,
    approval_contexts: Arc<std::sync::Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
    permission_refusal_reasons:
        Arc<std::sync::Mutex<HashMap<P::OperationId, ExternalProviderApprovalRefusalReason>>>,
    endpoint_id: Arc<tokio::sync::RwLock<Option<String>>>,
    #[cfg(any(test, feature = "test-observation"))]
    approval_refusal_warnings: Arc<std::sync::Mutex<Vec<ExternalProviderApprovalRefusalWarning>>>,
    #[cfg(any(test, feature = "test-observation"))]
    test_tool_calls: Arc<std::sync::Mutex<Vec<ExternalProviderToolCall>>>,
}

impl<P: InteractionPort> std::fmt::Debug for AgentSessionClient<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentSessionClient")
            .field("admission", &self.admission)
            .finish_non_exhaustive()
    }
}

impl<P: InteractionPort> AgentSessionClient<P> {
    pub async fn initialize(
        launch: ExternalProviderLaunch,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            INITIALIZE_TIMEOUT,
            Vec::new(),
            interaction_port,
            event_sink,
        )
        .await
    }

    pub async fn initialize_with_mcp_http(
        launch: ExternalProviderLaunch,
        server_name: impl Into<String>,
        server_url: impl Into<String>,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            INITIALIZE_TIMEOUT,
            vec![McpServer::Http(McpServerHttp::new(server_name, server_url))],
            interaction_port,
            event_sink,
        )
        .await
    }

    #[cfg(any(test, feature = "test-observation"))]
    pub async fn initialize_with_timeout(
        launch: ExternalProviderLaunch,
        initialize_timeout: Duration,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            initialize_timeout,
            Vec::new(),
            interaction_port,
            event_sink,
        )
        .await
    }

    async fn initialize_with_timeout_and_mcp_servers(
        launch: ExternalProviderLaunch,
        initialize_timeout: Duration,
        configured_mcp_servers: Vec<McpServer>,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
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
        let session_capabilities = Arc::new(tokio::sync::RwLock::new(HashMap::<
            String,
            ProviderCapabilityReport,
        >::new()));
        let task_session_capabilities = Arc::clone(&session_capabilities);
        #[cfg(any(test, feature = "test-observation"))]
        let permission_request_count = Arc::new(AtomicU64::new(0));
        #[cfg(any(test, feature = "test-observation"))]
        let callback_permission_request_count = Arc::clone(&permission_request_count);
        #[cfg(any(test, feature = "test-observation"))]
        let permission_outcome = Arc::new(std::sync::atomic::AtomicU8::new(0));
        #[cfg(any(test, feature = "test-observation"))]
        let callback_permission_outcome = Arc::clone(&permission_outcome);
        let callback_interaction_port = Arc::clone(&interaction_port);
        let task_interaction_port = Arc::clone(&interaction_port);
        let approval_contexts = Arc::new(std::sync::Mutex::new(HashMap::<
            String,
            ActiveApprovalContext<P>,
        >::new()));
        let callback_approval_contexts = Arc::clone(&approval_contexts);
        let task_approval_contexts = Arc::clone(&approval_contexts);
        let known_sessions = ProviderKnownSessions::default();
        let request_known_sessions = known_sessions.clone();
        let permission_refusal_reasons = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let callback_permission_refusal_reasons = Arc::clone(&permission_refusal_reasons);
        let endpoint_id = Arc::new(tokio::sync::RwLock::new(None::<String>));
        let callback_endpoint_id = Arc::clone(&endpoint_id);
        #[cfg(any(test, feature = "test-observation"))]
        let approval_refusal_warnings = Arc::new(std::sync::Mutex::new(Vec::<
            ExternalProviderApprovalRefusalWarning,
        >::new()));
        #[cfg(any(test, feature = "test-observation"))]
        let callback_approval_refusal_warnings = Arc::clone(&approval_refusal_warnings);
        #[cfg(any(test, feature = "test-observation"))]
        let test_tool_calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        #[cfg(any(test, feature = "test-observation"))]
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
            })
            .chain(futures_util::stream::once(async {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "provider stdout closed",
                ))
            }));
            let final_interaction_port = Arc::clone(&callback_interaction_port);
            let connection = Client.builder().name("codex-router-host")
                .with_handler(ProviderRequestSessionGuard::new(request_known_sessions))
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder, connection| {
                        #[cfg(any(test, feature = "test-observation"))]
                        callback_permission_request_count.fetch_add(1, Ordering::Relaxed);
                        let context = callback_approval_contexts.lock().ok().and_then(|contexts| {
                            contexts.get(request.session_id.0.as_ref()).cloned()
                        });
                        if context.is_none() {
                            let reason = ExternalProviderApprovalRefusalReason::MissingPromptContext;
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
                            #[cfg(any(test, feature = "test-observation"))]
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
                                refusals.insert(P::operation_id(&context.approval), reason);
                            }
                        }
                        spawn_external_approval_dispatch(
                            request,
                            responder,
                            connection,
                            context,
                            PermissionDispatchState {
                                interaction_port: Arc::clone(&callback_interaction_port),
                                refusal_reasons: Arc::clone(&callback_permission_refusal_reasons),
                                endpoint_id: Arc::clone(&callback_endpoint_id),
                                #[cfg(any(test, feature = "test-observation"))]
                                refusal_warnings: Arc::clone(&callback_approval_refusal_warnings),
                                #[cfg(any(test, feature = "test-observation"))]
                                permission_outcome: Arc::clone(&callback_permission_outcome),
                            },
                        )
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .with_handler(ProviderRequestFallback)
                .connect_with(
                Lines::new(outgoing, incoming),
                async move |connection| {
                    let initialized = tokio::select! {
                        biased;
                        () = task_shutdown.cancelled() => {
                            return Ok(());
                        }
                        response = initialize_provider_connection(&connection) => response,
                    };
                    let admission = match initialized {
                        Ok(response) if response.protocol_version == ProtocolVersion::V1 => {
                            let capability_report = ProviderCapabilityReport::from_initialize(&response);
                            Ok((ExternalProviderAdmission {
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
                            }, capability_report))
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
                        Ok((admission, _)) if admission.supports_mcp_http
                    ) {
                        configured_mcp_servers
                    } else {
                        Vec::new()
                    };
                    let base_capabilities = admission.as_ref().ok().map(|(_, report)| report.clone());
                    let _result = ready_tx.send(admission);
                    if admitted {
                        let mut sessions = HashMap::<
                            String,
                            tokio::sync::mpsc::Sender<ProviderSessionCommand<P>>,
                        >::new();
                        let mut session_tasks = tokio::task::JoinSet::new();
                        let mut admission_tasks = tokio::task::JoinSet::new();
                        let (admission_tx, mut admission_rx) = tokio::sync::mpsc::channel(32);
                        let mut pending_loads = std::collections::HashSet::<String>::new();
                        let Some(base_capabilities) = base_capabilities else { return Ok(()); };
                        loop {
                            tokio::select! {
                                () = task_shutdown.cancelled() => break,
                                completion = admission_rx.recv() => {
                                    let Some(completion) = completion else { continue; };
                                    match completion {
                                        PendingSessionAdmission::Create { result, reply } => {
                                            let report = result.as_ref().as_ref().ok().map(|registration| {
                                                base_capabilities.with_session_response(&registration.response)
                                            });
                                            let result = (*result).and_then(|registration| {
                                                register_provider_session(registration, &mut sessions)
                                            });
                                            if let Ok(created) = &result {
                                                known_sessions.track(created.provider_session_id.clone()).await;
                                                if let Some(report) = report {
                                                    task_session_capabilities.write().await.insert(created.provider_session_id.clone(), report);
                                                }
                                            }
                                            let _result = reply.send(result);
                                        }
                                        PendingSessionAdmission::Load { provider_session_id, result, reply } => {
                                            pending_loads.remove(&provider_session_id);
                                            let report = result.as_ref().as_ref().ok().map(|session| {
                                                base_capabilities.with_session_response(&session.response())
                                            });
                                            let result = (*result).and_then(|mut session| {
                                                discard_queued_session_updates(&mut session)?;
                                                register_static_provider_session(
                                                    session,
                                                    &mut sessions,
                                                    &mut session_tasks,
                                                    task_shutdown.clone(),
                                                    Arc::clone(&task_frame_observation),
                                                    #[cfg(any(test, feature = "test-observation"))] Arc::clone(&session_test_tool_calls),
                                                )
                                            }).map(|_| ());
                                            if result.is_err() {
                                                known_sessions.forget(&provider_session_id).await;
                                            } else if let Some(report) = report {
                                                task_session_capabilities.write().await.insert(provider_session_id.clone(), report);
                                            }
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
                                            #[cfg(any(test, feature = "test-observation"))]
                                            let pending_test_tool_calls = Arc::clone(&session_test_tool_calls);
                                            admission_tasks.spawn(async move {
                                                run_create_admission(
                                                    pending_connection,
                                                    request,
                                                    pending_shutdown,
                                                    pending_admission_tx,
                                                    reply,
                                                    pending_frame_observation,
                                                    #[cfg(any(test, feature = "test-observation"))] pending_test_tool_calls,
                                                )
                                                .await;
                                            });
                                        }
                                        ProviderCommand::Load { provider_session_id, cwd, reply } => {
                                            if sessions.contains_key(&provider_session_id) || !pending_loads.insert(provider_session_id.clone()) {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalBusy));
                                            } else {
                                                known_sessions.track(provider_session_id.clone()).await;
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
                                                        .start_session() => result.map(|restored| restored.into_session()).map_err(acp_load_session_error),
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
                                            let turn_cancellation = operation_id.as_ref().and_then(|operation_id| active_turn_cancellation(&task_approval_contexts, &task_interaction_port, &provider_session_id, Some(operation_id)));
                                            if let Err(error) = session.send(ProviderSessionCommand::Prompt { operation_id, prompt, turn_cancellation, dispatch, reply }).await
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
            final_interaction_port.cancel_retired().await;
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

        let (admission, base_capabilities) =
            match tokio::time::timeout(initialize_timeout, ready_rx).await {
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
            base_capabilities,
            session_capabilities,
            shutdown,
            retirement,
            task: tokio::sync::Mutex::new(Some(task)),
            shutdown_failed,
            frame_observation,
            commands: command_tx,
            #[cfg(any(test, feature = "test-observation"))]
            permission_request_count,
            #[cfg(any(test, feature = "test-observation"))]
            permission_outcome,
            interaction_port,
            _event_sink: event_sink,
            approval_contexts,
            permission_refusal_reasons,
            endpoint_id,
            #[cfg(any(test, feature = "test-observation"))]
            approval_refusal_warnings,
            #[cfg(any(test, feature = "test-observation"))]
            test_tool_calls,
        })
    }
}

impl<P: InteractionPort> Drop for AgentSessionClient<P> {
    fn drop(&mut self) {
        self.retirement.cancel();
        self.shutdown.cancel();
    }
}
