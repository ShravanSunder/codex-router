//! ACP provider process admission and connection lifetime.

mod approval_turn_cancellation;
mod external_approval_dispatch;
mod provider_acp_error_mapping;
mod provider_approval_dispatch;
mod provider_auth_status_update;
mod provider_client_contract;
mod provider_client_initialization;
mod provider_client_operations;
mod provider_connection_task;
mod provider_cursor_create_plan;
mod provider_cursor_plan_items;
mod provider_cursor_question;
mod provider_form_elicitation;
mod provider_frame_observation;
mod provider_initialize_request;
mod provider_lifecycle_requests;
mod provider_prompt_dispatch;
mod provider_request_fallback;
mod provider_session_admission;
mod provider_session_restore;
pub(crate) mod provider_setting_application;

use crate::ProviderCapabilityReport;
use crate::provider_connection_activity::{ProviderConnectionActivity, ToolCallOwnershipHandler};
use crate::provider_prompt_content::ProviderPromptContent;
use crate::provider_session_actor::{
    ProviderPromptDispatchObservation, ProviderSessionActivity, ProviderSessionCommand,
    ProviderSessionRuntimeHandles, ProviderSteeringOutcome, run_provider_session,
};
use crate::{AcpProtocolVersion, InteractionPort, SessionEventSink};
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
use external_approval_dispatch::{
    PermissionDispatchState, record_permission_refusal, spawn_external_approval_dispatch,
};
use provider_acp_error_mapping::acp_load_session_error;
pub(crate) use provider_acp_error_mapping::acp_operation_error;
use provider_auth_status_update::ProviderAuthStatusHandler;
#[cfg(any(test, feature = "test-observation"))]
pub(crate) use provider_client_contract::classify_mcp_tool_outcome;
#[cfg(feature = "test-observation")]
pub(crate) use provider_client_contract::sanitized_acp_error;
pub use provider_client_contract::{
    ExternalProviderAdmission, ExternalProviderApprovalRefusalReason,
    ExternalProviderCreatedSession, ExternalProviderLaunch, ExternalProviderPromptOutcome,
    ExternalProviderRuntimeError, ProviderSessionSummary,
};
#[cfg(any(test, feature = "test-observation"))]
pub use provider_client_contract::{
    ExternalProviderApprovalRefusalWarning, ExternalProviderPermissionObservation,
    ExternalProviderPermissionOutcome, ExternalProviderToolCall, ExternalProviderToolOutcome,
};
pub(crate) use provider_client_contract::{
    provider_frame_decode_error, sanitized_initialization_error,
};
use provider_cursor_create_plan::ProviderCursorCreatePlanHandler;
pub(crate) use provider_cursor_plan_items::CursorPlanItems;
use provider_cursor_plan_items::ProviderCursorTodoHandler;
use provider_cursor_question::ProviderCursorQuestionHandler;
use provider_form_elicitation::ProviderFormElicitationHandler;
pub(crate) use provider_frame_observation::ProviderFrameObservation;
use provider_initialize_request::initialize_provider_connection;
use provider_request_fallback::{
    ProviderKnownSessions, ProviderRequestFallback, ProviderRequestSessionGuard,
};
use provider_session_admission::*;
use session_event_model::InputId;
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
pub(crate) const MAX_ACP_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_PROMPT_OUTPUT_BYTES: usize = 1024 * 1024;

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

enum ProviderCommand<P: InteractionPort> {
    Create {
        cwd: PathBuf,
        settings: crate::RequestedProviderSettings,
        reply: tokio::sync::oneshot::Sender<
            Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError>,
        >,
    },
    Restore {
        provider_session_id: String,
        cwd: PathBuf,
        mode: provider_session_restore::RestoreHistoryMode,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    List {
        cwd: Option<PathBuf>,
        reply: tokio::sync::oneshot::Sender<
            Result<Vec<ProviderSessionSummary>, ExternalProviderRuntimeError>,
        >,
    },
    Close {
        provider_session_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    Prompt {
        provider_session_id: String,
        input_id: InputId,
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
        input_id: InputId,
        prompt: ProviderPromptContent,
        reply: tokio::sync::oneshot::Sender<
            Result<ProviderSteeringOutcome<P::OperationId>, ExternalProviderRuntimeError>,
        >,
    },
    InspectSession {
        provider_session_id: String,
        reply: tokio::sync::oneshot::Sender<ProviderSessionActivity>,
    },
    InspectActiveOperation {
        provider_session_id: String,
        reply: tokio::sync::oneshot::Sender<Option<P::OperationId>>,
    },
    WaitSessionIdle {
        provider_session_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    SetSetting {
        provider_session_id: String,
        kind: crate::ProviderSettingKind,
        value: String,
        reply: tokio::sync::oneshot::Sender<
            Result<crate::EffectiveProviderSettings, ExternalProviderRuntimeError>,
        >,
    },
}

enum PendingSessionAdmission<P: InteractionPort> {
    Create {
        result: Box<Result<ProviderSessionRegistration<P>, ExternalProviderRuntimeError>>,
        activation: tokio::sync::oneshot::Sender<()>,
        reply: tokio::sync::oneshot::Sender<
            Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError>,
        >,
    },
    Restore {
        provider_session_id: String,
        result: Box<Result<RestoredProviderSession, ExternalProviderRuntimeError>>,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    Close {
        provider_session_id: String,
        result: Result<(), ExternalProviderRuntimeError>,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
}

struct ProviderSessionRegistration<P: InteractionPort> {
    provider_session_id: String,
    commands: tokio::sync::mpsc::Sender<ProviderSessionCommand<P>>,
    response: NewSessionResponse,
    settings_catalog: crate::ProviderSettingsCatalog,
    setup_error: Option<ExternalProviderRuntimeError>,
}

struct RestoredProviderSession {
    session: ActiveSession<'static, Agent>,
    item_projection: Option<crate::provider_item_projection::ProviderItemProjection>,
    settings_catalog: crate::ProviderSettingsCatalog,
}

/// Owns the provider process and ACP connection independently of caller tasks.
pub struct AgentSessionClient<P: InteractionPort> {
    admission: ExternalProviderAdmission,
    base_capabilities: ProviderCapabilityReport,
    session_capabilities: Arc<tokio::sync::RwLock<HashMap<String, ProviderCapabilityReport>>>,
    auth_status: Arc<tokio::sync::RwLock<session_event_model::ProviderAuthStatus>>,
    session_settings: Arc<tokio::sync::RwLock<HashMap<String, crate::ProviderSettingsCatalog>>>,
    last_settings_catalog: Arc<tokio::sync::RwLock<Option<crate::ProviderSettingsCatalog>>>,
    settings_unresolved: Arc<tokio::sync::RwLock<HashMap<String, crate::ProviderSettingKind>>>,
    shutdown: CancellationToken,
    sink_closed: CancellationToken,
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

impl<P: InteractionPort> Drop for AgentSessionClient<P> {
    fn drop(&mut self) {
        self.retirement.cancel();
        self.shutdown.cancel();
    }
}
