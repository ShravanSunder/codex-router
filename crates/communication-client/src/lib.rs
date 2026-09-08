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
