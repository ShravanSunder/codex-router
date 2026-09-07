//! ACP conversations translated into the existing shared Codex runtime.
mod acp_connection_dispatch;
mod acp_connection_transport;
mod acp_frame_transport;
mod assistant_text_projection;
mod connection_negotiation;
mod history_projection;
mod mcp_configuration;
mod native_prompt_execution;
mod permission_address;
mod permission_translation;
mod prompt_connection_task;
mod prompt_content_translation;
mod prompt_settlement;
mod queued_frame_budget;
mod session_connection_registry;
mod session_creation;
mod stored_session_listing;
pub use acp_connection_dispatch::{AcpConnectionInputs, AcpStoredSessions, serve_acp_connection};
pub use acp_connection_transport::{
    AcpRouterChannels, AcpWireChannels, acp_connection_channels, run_acp_transport,
};
pub use acp_frame_transport::{read_acp_frame, write_acp_frame};
pub use prompt_connection_task::{
    PromptCommand, PromptTaskCompletion, PromptTaskInputs, run_prompt_task,
};
pub use queued_frame_budget::{AcpOutputSender, QueuedAcpFrame, bounded_acp_output};
pub use session_connection_registry::{AcpSessionRegistry, SessionRegistryError};
pub use stored_session_listing::NativeStoredSessions;
mod tool_progress_projection;
pub use assistant_text_projection::{PromptTarget, TextProjectionError, project_assistant_text};
pub use communication_protocol::{
    ACP_SCHEMA_BYTES, ACP_SCHEMA_DIGEST, AcpSchemaCatalog, AcpSchemaError,
};
pub use connection_negotiation::{AcpNegotiation, AcpNegotiationError};
pub use history_projection::{HistoryProjectionError, project_history};
pub use mcp_configuration::{McpConfiguration, McpConfigurationError};
pub use native_prompt_execution::{PendingAcpPrompt, PromptEvent, PromptExecutionError};
pub use permission_translation::{PendingPermission, PermissionReply, PermissionTranslationError};
pub use prompt_content_translation::{
    PromptContentError, TranslatedPrompt, translate_prompt_content,
};
pub use prompt_settlement::{NativeInterruptionState, NativePromptTerminal, PromptSettlement};
pub use session_creation::{AcpSessionBinding, SessionSetupError, SessionSetupInputs};
pub use tool_progress_projection::{ToolProjectionError, project_tool_progress};

mod session_setup_task;

mod cancellation_barrier;
pub use cancellation_barrier::CancellationBarrier;
