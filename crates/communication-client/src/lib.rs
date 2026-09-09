//! Rust client for the public owner-local communication protocol.
mod control_connection;
mod endpoint_notification_state;
pub use communication_protocol::{ControlInitializationResult, EndpointInventory, ProtocolVersion};
pub use control_connection::{ClientError, ControlClient};
mod service_discovery;

pub use communication_protocol::JournalStatus;

mod native_endpoint_selector;
pub use native_endpoint_selector::resolve_public_native;

mod observation_session;
pub use observation_session::NativeObservation;

mod acp_conversation;
pub use acp_conversation::{AcpConversation, ConversationEnd, ConversationEvent};
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
