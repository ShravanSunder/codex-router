//! Claude Code's owner-local session registry and peer socket transport.
mod claude_code_peer_socket;
mod claude_code_session_registry;

pub use claude_code_peer_socket::{ClaudeCodePeerSocket, PeerSocketWriteOutcome};
pub use claude_code_session_registry::{
    ClaudeCodeSessionRegistry, PeerSessionLookup, PeerSessionRecord, PeerSessionStatus,
};
