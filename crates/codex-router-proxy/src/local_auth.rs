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

/// Extracts the local router token from supported Codex provider auth headers.
#[must_use]
pub(crate) fn presented_local_token<'a>(
    router_token_header: Option<&'a str>,
    authorization_header: Option<&'a str>,
) -> Option<&'a str> {
    if router_token_header.is_some() {
        return router_token_header;
    }

    bearer_token_from_authorization(authorization_header?)
}

fn bearer_token_from_authorization(value: &str) -> Option<&str> {
    let value = value.trim();
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }

    let token = token.trim();
    if token.is_empty() {
        return None;
    }

    Some(token)
}

#[cfg(test)]
mod tests {
    use super::bearer_token_from_authorization;
    use super::presented_local_token;

    #[test]
    fn presented_local_token_prefers_explicit_router_token_header() {
        assert_eq!(
            presented_local_token(Some("router-token"), Some("Bearer authorization-token")),
            Some("router-token")
        );
    }

    #[test]
    fn presented_local_token_accepts_authorization_bearer() {
        assert_eq!(
            presented_local_token(None, Some("Bearer router-token")),
            Some("router-token")
        );
        assert_eq!(
            presented_local_token(None, Some("bearer router-token")),
            Some("router-token")
        );
    }

    #[test]
    fn bearer_token_from_authorization_ignores_non_bearer_auth() {
        assert_eq!(bearer_token_from_authorization("Basic abc"), None);
        assert_eq!(bearer_token_from_authorization("Bearer"), None);
        assert_eq!(bearer_token_from_authorization("Bearer "), None);
    }
}
