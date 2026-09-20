//! Streamable HTTP MCP transport for the collaboration SDK.

mod mcp_http_listener;
mod mcp_server;
#[cfg(test)]
mod permission_entry_path_tests;

pub use mcp_http_listener::{
    CollaborationMcpListener, CollaborationMcpListenerConfig, LoopbackBindAddress,
    LoopbackBindAddressError,
};
