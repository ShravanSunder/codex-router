//! Owner-local communication service, independent of the Host executable.
mod control_connection;
pub use control_connection::{ServiceIdentity, serve_control_connection};
mod endpoint_directory;
pub use endpoint_directory::{
    EndpointDirectory, EndpointSnapshot, EndpointSubscription, EndpointUpdate,
};
mod local_control_listener;
pub use local_control_listener::LocalControlService;
mod native_channel_relay;
pub use native_channel_relay::{
    MAX_NATIVE_MESSAGE_BYTES, connect_native_relay, relay_native_channels,
};
mod native_generation_gate;
pub use native_generation_gate::{NativeAdmission, NativeGenerationGate};
mod native_relay_listener;
mod private_socket_listener;
pub use native_relay_listener::NativeRelayListener;
mod service_identity_storage;
pub use service_identity_storage::{load_service_identity, new_service_uuid};
mod manifest_publication;
pub use manifest_publication::ManifestPublication;
mod control_schema_publication;
mod journal_dispatch;
pub use control_schema_publication::publish_control_schema;
mod native_control_dispatch;
pub use native_control_dispatch::NativeControlBackend;

mod agent_declaration;
mod message_effect_state;
mod native_message_dispatch;

mod acp_channel_listener;
pub use acp_channel_listener::AcpChannelListener;
pub use codex_acp_adapter::{ACP_SCHEMA_DIGEST, NativeStoredSessions};

mod instruction_dispatch;
mod session_inventory_dispatch;
mod stored_inventory_observation;

mod wakeup_dispatch;
mod wakeup_projection;
mod wakeup_timing_worker;
pub use wakeup_timing_worker::WakeTimingWorker;

mod delivery_projection;
mod wakeup_native_sender;

mod wakeup_lifecycle_dispatch;

mod wakeup_subscription;

mod wakeup_list_dispatch;

mod schedule_dispatch;
mod schedule_projection;

mod native_thread_preparation;
mod schedule_preparation_dispatch;
