//! Validated opt-in exposure of the existing collaboration MCP application.

use std::num::NonZeroU16;

/// One Host invocation's private remote-access choice. Omission remains local-only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteAccess {
    Tailscale { https_port: NonZeroU16 },
}

impl RemoteAccess {
    #[must_use]
    pub const fn tailscale(https_port: NonZeroU16) -> Self {
        Self::Tailscale { https_port }
    }
}
