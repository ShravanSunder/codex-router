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

/// Extracts the local router token from supported local auth carriers.
pub(crate) fn extract_presented_local_token<'a>(
    router_token_header: Option<&'a str>,
    authorization_header: Option<&'a str>,
) -> Result<Option<&'a str>, LocalAuthError> {
    let router_token = non_empty_trimmed(router_token_header);
    let bearer_token = authorization_header.and_then(bearer_token_from_authorization);
    match (router_token, bearer_token) {
        (Some(router_token), Some(bearer_token)) if router_token != bearer_token => {
            Err(LocalAuthError::Wrong)
        }
        (Some(router_token), Some(_)) => Ok(Some(router_token)),
        (Some(router_token), None) => Ok(Some(router_token)),
        (None, Some(bearer_token)) => Ok(Some(bearer_token)),
        (None, None) => Ok(None),
    }
}

/// Extracts local auth and rejects forbidden smuggling carriers.
pub(crate) fn extract_presented_local_token_from_request<'a>(
    router_token_header: Option<&'a str>,
    authorization_header: Option<&'a str>,
    cookie_header: Option<&str>,
    path: &str,
    body: &[u8],
    inspect_json_body: bool,
) -> Result<Option<&'a str>, LocalAuthError> {
    if has_forbidden_query_auth_carrier(path)
        || has_forbidden_cookie_auth_carrier(cookie_header)
        || (inspect_json_body && has_forbidden_top_level_json_auth_carrier(body))
    {
        return Err(LocalAuthError::Wrong);
    }

    extract_presented_local_token(router_token_header, authorization_header)
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

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    if value.is_empty() { None } else { Some(value) }
}

fn has_forbidden_query_auth_carrier(path: &str) -> bool {
    let Some((_path, query)) = path.split_once('?') else {
        return false;
    };

    query.split(['&', ';']).any(|pair| {
        let name = pair.split_once('=').map_or(pair, |(name, _value)| name);
        canonical_auth_field_name(name)
            .as_deref()
            .is_some_and(is_forbidden_auth_field_name)
    })
}

fn has_forbidden_cookie_auth_carrier(cookie_header: Option<&str>) -> bool {
    let Some(cookie_header) = cookie_header else {
        return false;
    };
    !cookie_header.trim().is_empty()
}

pub(crate) fn has_forbidden_top_level_json_auth_carrier(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return body_mentions_forbidden_auth_carrier(body);
    };
    let Some(object) = value.as_object() else {
        return body_mentions_forbidden_auth_carrier(body);
    };

    object
        .keys()
        .filter_map(|key| canonical_auth_field_name(key))
        .any(|key| is_forbidden_auth_field_name(&key))
}

fn body_mentions_forbidden_auth_carrier(body: &[u8]) -> bool {
    let body = String::from_utf8_lossy(body);
    body.split(|character: char| {
        !(character.is_ascii_alphanumeric()
            || character == '%'
            || character == '-'
            || character == '_'
            || character == '.')
    })
    .filter_map(canonical_auth_field_name)
    .any(|field_name| is_forbidden_auth_field_name(&field_name))
}

fn canonical_auth_field_name(name: &str) -> Option<String> {
    let decoded = percent_decode(name)?;
    Some(
        decoded
            .trim()
            .chars()
            .map(|character| match character {
                '-' | '_' | '.' | ' ' => '_',
                other => other.to_ascii_lowercase(),
            })
            .collect::<String>(),
    )
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            output.push(hex_value(high)? << 4 | hex_value(low)?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(output).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_forbidden_auth_field_name(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "x-codex-router-token"
            | "x_codex_router_token"
            | "codex-router-token"
            | "codex_router_token"
            | "access_token"
            | "refresh_token"
            | "api_key"
            | "bearer"
            | "token"
    )
}

#[cfg(test)]
mod tests {
    use super::bearer_token_from_authorization;
    use super::extract_presented_local_token;
    use super::extract_presented_local_token_from_request;
    use codex_router_core::local_auth::LocalAuthError;

    #[test]
    fn presented_local_token_accepts_equal_mixed_carriers() {
        assert_eq!(
            extract_presented_local_token(Some("router-token"), Some("Bearer router-token")),
            Ok(Some("router-token"))
        );
        assert_eq!(
            extract_presented_local_token(Some("router-token"), Some("Bearer other")),
            Err(LocalAuthError::Wrong)
        );
    }

    #[test]
    fn presented_local_token_accepts_authorization_bearer() {
        assert_eq!(
            extract_presented_local_token(None, Some("Bearer router-token")),
            Ok(Some("router-token"))
        );
        assert_eq!(
            extract_presented_local_token(None, Some("bearer router-token")),
            Ok(Some("router-token"))
        );
    }

    #[test]
    fn bearer_token_from_authorization_ignores_non_bearer_auth() {
        assert_eq!(bearer_token_from_authorization("Basic abc"), None);
        assert_eq!(bearer_token_from_authorization("Bearer"), None);
        assert_eq!(bearer_token_from_authorization("Bearer "), None);
    }

    #[test]
    fn request_local_auth_rejects_forbidden_smuggling_carriers() {
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses?token=router-token",
                b"{}",
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses?ok=1;token=router-token",
                b"{}",
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses?authoriz%61tion=router-token",
                b"{}",
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses?x.codex.router.token=router-token",
                b"{}",
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                Some("session=router-token"),
                "/v1/responses",
                b"{}",
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses?refresh_token=router-token",
                b"{}",
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses",
                br#"{"x-codex-router-token":"router-token"}"#,
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses",
                br#"{"bearer":"router-token"}"#,
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses",
                br#"{"refresh_token":"router-token"}"#,
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses",
                br#"{"token":"router-token""#,
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses",
                br#""token=router-token""#,
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses",
                br#"{"x.codex.router.token":"router-token"}"#,
                true,
            ),
            Err(LocalAuthError::Wrong)
        );
    }

    #[test]
    fn request_local_auth_allows_nested_prompt_canaries() {
        assert_eq!(
            extract_presented_local_token_from_request(
                Some("router-token"),
                None,
                None,
                "/v1/responses",
                br#"{"prompt":{"x-codex-router-token":"nested-token"}}"#,
                true,
            ),
            Ok(Some("router-token"))
        );
    }
}
