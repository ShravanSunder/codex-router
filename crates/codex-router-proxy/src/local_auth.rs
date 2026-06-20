//! Proxy-facing local auth gate.

use std::sync::Arc;
use std::sync::RwLock;

use codex_router_core::ids::TokenGeneration;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::local_auth::LocalRouterAuth;

/// Local auth gate that runs before selection or upstream open.
#[derive(Clone, Debug)]
pub struct ProxyLocalAuthGate {
    auth: Arc<RwLock<LocalRouterAuth>>,
}

impl ProxyLocalAuthGate {
    /// Builds a proxy auth gate.
    #[must_use]
    pub fn new(auth: LocalRouterAuth) -> Self {
        Self {
            auth: Arc::new(RwLock::new(auth)),
        }
    }

    /// Authorizes a presented local token.
    pub fn authorize(
        &self,
        presented_token: Option<&str>,
    ) -> Result<TokenGeneration, LocalAuthError> {
        let auth = self.auth.read().map_err(|_error| LocalAuthError::Wrong)?;

        auth.validate(presented_token)
    }

    /// Replaces the active auth snapshot.
    pub fn replace(&self, auth: LocalRouterAuth) {
        if let Ok(mut current_auth) = self.auth.write() {
            *current_auth = auth;
        }
    }

    /// Returns whether a generation still matches the current token snapshot.
    #[must_use]
    pub fn is_current_generation(&self, generation: TokenGeneration) -> bool {
        self.auth
            .read()
            .map(|auth| auth.is_current_generation(generation))
            .unwrap_or(false)
    }
}
