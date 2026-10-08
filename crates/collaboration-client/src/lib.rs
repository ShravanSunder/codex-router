//! Reusable collaboration SDK: typed calls to the collaboration API and the carrier clients.
/// Shared request, result, identity and schema definitions used by the service.
pub use collaboration_protocol as protocol;
/// Message-board request, result and validated domain types.
pub use message_board as board;
mod admission_overload;
mod api_connection;
mod collaboration_access;
mod collaboration_client;
mod message_operation;
mod provider_conversations;
pub use collaboration_access::{CollaborationAccess, LocalCollaboration, LocalFuture};
pub use provider_conversations::ProviderConversations;
mod operation_error;
pub use collaboration_client::{ClientError, CollaborationClient, RouterIdentity};
pub use collaboration_protocol::EndpointInventory;
pub use collaboration_protocol::{
    AdapterOperationFailure as OperationFailure, OperationEffect, OperationFailureKind,
};
pub use collaboration_protocol::{
    PushRecordHistoryParams, PushRecordListParams, PushRecordShowParams,
};
pub use message_operation::{
    MessageReplyError, MessageReplyRequest, MessageSendError, MessageSendRequest,
    PublicMessageContent,
};
pub use operation_error::{OperationError, operation_failure_from_client_error};
static OBSERVED_SERVICE_VERSION: std::sync::OnceLock<std::sync::Mutex<String>> =
    std::sync::OnceLock::new();
pub(crate) fn record_service_version(version: &str) {
    if let Ok(mut stored) = OBSERVED_SERVICE_VERSION
        .get_or_init(|| std::sync::Mutex::new(String::new()))
        .lock()
    {
        *stored = version.to_owned();
    }
}
/// The service version this process read from the manifest, or `None` before any
/// client connected. An empty string is not a version.
#[must_use]
pub fn observed_service_version() -> Option<String> {
    OBSERVED_SERVICE_VERSION
        .get()
        .and_then(|value| value.lock().ok().map(|version| version.clone()))
        .filter(|version| !version.is_empty())
}

static ROUTER_VERSION_WARNING_PRINTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) fn warn_on_router_version_mismatch(
    client_name: &str,
    client_version: &str,
    service_version: &str,
) {
    let executable_name = std::env::current_exe().ok().and_then(|path| {
        path.file_name()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_owned)
    });
    if let Some(warning) = router_version_warning(
        client_name,
        client_version,
        service_version,
        executable_name.as_deref(),
    ) && !ROUTER_VERSION_WARNING_PRINTED.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        eprintln!("{warning}");
    }
}

fn router_version_warning(
    client_name: &str,
    client_version: &str,
    service_version: &str,
    executable_name: Option<&str>,
) -> Option<String> {
    ((client_name == "agent-collaboration"
        || executable_name == Some("agent-collaboration"))
        && !service_version.is_empty()
        && client_version != service_version)
        .then(|| {
            format!(
                "⚠ Router Host is stale (running {service_version}, CLI {client_version}); run `codex-router host restart`"
            )
        })
}

/// The scripted API the integration tests also use, for this crate's unit tests.
#[cfg(test)]
#[path = "../tests/support/scripted_api.rs"]
mod scripted_api;

#[cfg(test)]
mod router_version_warning_tests {
    use super::router_version_warning;

    #[test]
    fn only_agent_collaboration_version_mismatch_warns() {
        assert_eq!(
            router_version_warning("agent-collaboration", "0.1.37", "0.1.36", None).as_deref(),
            Some(
                "⚠ Router Host is stale (running 0.1.36, CLI 0.1.37); run `codex-router host restart`"
            )
        );
        assert_eq!(
            router_version_warning("other-client", "0.1.37", "0.1.36", None),
            None
        );
        assert!(
            router_version_warning(
                "conversation-client",
                "0.1.37",
                "0.1.36",
                Some("agent-collaboration")
            )
            .is_some()
        );
        assert_eq!(
            router_version_warning("agent-collaboration", "0.1.37", "0.1.37", None),
            None
        );
    }
}

pub use collaboration_protocol::JournalStatus;

mod native_endpoint_selector;
pub use native_endpoint_selector::resolve_public_native;

mod observation_session;
mod provider_session_observation;
pub use collaboration_protocol::{
    BoundedObservationRequest, BoundedObservationResult, ObservationEndReason,
};
pub use observation_session::NativeObservation;
pub use provider_session_observation::SessionObservation;

mod acp_conversation;
pub use acp_conversation::AcpConversation;
mod conversation_client;
mod conversation_contract;
mod conversation_create_actor;
mod conversation_operation_result;
mod conversation_session_operations;
mod owner_identity_resolution;
pub use conversation_client::{
    ConversationCancelInput, ConversationClient, ConversationClientError, ConversationCreateInput,
    ConversationCreatePromptInput, ConversationLoadInput, ConversationPromptInput,
};
pub use conversation_create_actor::ConversationCreateActor;
pub use conversation_operation_result::{
    ConversationCreatePromptOutcome, ConversationOperationResult, ConversationSettlement,
    ConversationSettlementDetail, ConversationStopReason, ProviderLoadOutput, ProviderPromptOutput,
};
pub use owner_identity_resolution::{OwnerIdentityError, resolve_owner_human_id};
mod provider_conversation_operations;
pub use conversation_contract::{
    ConversationCreatePromptError, ConversationCreatePromptRequest, ConversationCreatePromptResult,
    ConversationCreateRequest, ConversationCreateResult, ConversationEnd, ConversationEvent,
    ConversationPromptRequest, ExistingConversationPromptError, ExistingConversationPromptRequest,
    ExistingConversationPromptResult, PublicPromptContent,
};
mod acp_transport_connection;
pub use acp_transport_connection::AcpTransportConnection;
mod instruction_operations;
pub use instruction_operations::InstructionClientError;
mod wakeup_operations;
pub use wakeup_operations::WakeClientError;

mod wakeup_waiting;
pub use wakeup_waiting::{WakeWaitConnection, WakeWaitError};
pub use wakeup_waiting::{WakeWaitFailure, WakeWaitFailureKind};
mod automation_inspection_operations;
mod schedule_operations;
pub use automation_inspection_operations::AutomationInspectionClientError;
pub use schedule_operations::ScheduleClientError;
mod run_operations;
pub use run_operations::RunClientError;
mod automation_configuration_operations;
pub use automation_configuration_operations::ConfigurationClientError;

mod board_operations;
pub use board_operations::BoardClientError;

mod board_repository;
pub use board_repository::{BoardRepositoryError, BoardRepositoryLocation};

mod service_directory;
pub use service_directory::{
    COLLABORATION_SERVICE_DIRECTORY_NAME, ServiceDirectoryError, ServiceDirectoryOptions,
    resolve_service_directory,
};
mod native_transport_connection;
pub use native_transport_connection::{
    NATIVE_WEBSOCKET_FRAME_LIMIT, NativeTransportConnection, NativeTransportError,
};

pub mod session_catalog;
