//! Static compatibility contract for host-managed router discovery.

use serde::Deserialize;
use serde::Serialize;

/// Compatibility revision understood by this router release.
pub const ROUTER_COMPATIBILITY_REVISION: u16 = 1;

/// Static loopback router identity returned by `GET /healthz`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RouterCompatibility {
    /// Product identity expected by the shared host.
    pub product: String,
    /// Same-release compatibility revision.
    pub compatibility_revision: u16,
    /// Router binary version.
    pub binary_version: String,
    /// Whether model routes require a local bearer token.
    pub local_model_authentication_required: bool,
}

impl RouterCompatibility {
    /// Builds the static response for the running router configuration.
    #[must_use]
    pub fn current(local_model_authentication_required: bool) -> Self {
        Self {
            product: "codex-router".to_owned(),
            compatibility_revision: ROUTER_COMPATIBILITY_REVISION,
            binary_version: env!("CARGO_PKG_VERSION").to_owned(),
            local_model_authentication_required,
        }
    }
}
