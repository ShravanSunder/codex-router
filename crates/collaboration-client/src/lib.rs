//! Reusable collaboration SDK: operations and the shared RPC contract.
/// Shared request, result, identity and schema definitions used by the service.
pub use collaboration_protocol as protocol;
/// Message-board request, result and validated domain types.
pub use message_board as board;
mod control_connection;
mod endpoint_notification_state;
pub use collaboration_protocol::{ControlInitializationResult, EndpointInventory, ProtocolVersion};
pub use control_connection::{ClientError, ControlClient};
mod service_discovery;

pub use collaboration_protocol::JournalStatus;

mod native_endpoint_selector;
pub use native_endpoint_selector::resolve_public_native;

mod observation_session;
pub use observation_session::NativeObservation;

mod acp_conversation;
pub use acp_conversation::{
    AcpConversation, ConversationEnd, ConversationEvent, ConversationSessionRequest,
};
mod acp_transport_connection;
pub use acp_transport_connection::AcpTransportConnection;
mod instruction_operations;
pub use instruction_operations::InstructionClientError;
mod wakeup_operations;
pub use wakeup_operations::WakeClientError;

mod wakeup_waiting;
pub use wakeup_waiting::{WakeWaitConnection, WakeWaitError};
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
