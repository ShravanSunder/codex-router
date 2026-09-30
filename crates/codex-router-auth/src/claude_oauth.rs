//! Router-owned Claude subscription OAuth login and refresh.

use std::fmt;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::credential_bundle::CredentialBundle;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest as _;
use sha2::Sha256;
use thiserror::Error;

use crate::resolver::CredentialRefreshClient;
pub use crate::resolver::CredentialRefreshFailure;
pub use codex_router_state::credential_maintenance::CredentialFailureClass;

/// Public Claude Code OAuth client identifier audited from the open source implementations.
pub const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// Anthropic-hosted OAuth callback used by Claude Code.
pub const CLAUDE_OAUTH_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
/// Claude Code OAuth scopes from the pinned CLIProxyAPI and Better CCflare audits.
pub const CLAUDE_OAUTH_SCOPE: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

const CLAUDE_OAUTH_AUTHORIZE_ENDPOINT: &str = "https://claude.ai/oauth/authorize";
const CLAUDE_OAUTH_TOKEN_ENDPOINT: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_OAUTH_CLIENT_ID_ENV: &str = "CODEX_ROUTER_CLAUDE_OAUTH_CLIENT_ID";
const CLAUDE_OAUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Typed Claude account login flow used by provider-specific CLI dispatch.
pub trait AccountLoginFlow {
    /// Pending state and PKCE verifier retained only for this login attempt.
    type PendingLogin;

    /// Begins an authorization flow and returns the state needed for its URL.
    fn begin_login(&self) -> Result<Self::PendingLogin, LoginFlowError>;

    /// Builds the hosted authorize URL for the pending flow.
    fn authorization_url(&self, pending: &Self::PendingLogin) -> Result<String, LoginFlowError>;

    /// Exchanges a pasted `code#state` callback and returns tokens in memory.
    fn finish_login(
        &self,
        pending: Self::PendingLogin,
        pasted_callback: &str,
    ) -> Result<CredentialBundle, LoginFlowError>;
}

/// Secret-safe failures at the Claude login boundary.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum LoginFlowError {
    #[error("could not create secure OAuth state")]
    SecureRandomUnavailable,
    #[error("Claude OAuth callback must be a non-empty code#state value")]
    InvalidCallback,
    #[error("Claude OAuth callback state did not match this login")]
    StateMismatch,
    #[error("Claude OAuth authorization request failed")]
    AuthorizationRequestFailed,
    #[error("Claude OAuth provider refused the authorization code")]
    ProviderRefused,
    #[error("Claude OAuth provider returned an invalid token response")]
    InvalidTokenResponse,
    #[error("system clock unavailable for OAuth token expiry")]
    ClockUnavailable,
}

/// In-memory PKCE state for one Claude authorization attempt.
pub struct PendingClaudeOAuthLogin {
    state: String,
    code_verifier: String,
}

impl fmt::Debug for PendingClaudeOAuthLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingClaudeOAuthLogin")
            .field("state", &"[REDACTED]")
            .field("code_verifier", &"[REDACTED]")
            .finish()
    }
}

/// Claude subscription OAuth authorization-code and refresh client.
#[derive(Clone, Debug)]
pub struct ClaudeOAuthLoginFlow {
    token_endpoint: String,
    client_id: String,
    request_timeout: Duration,
}

impl ClaudeOAuthLoginFlow {
    /// Creates a login flow using Anthropic's public Claude Code OAuth client.
    /// `CODEX_ROUTER_CLAUDE_OAUTH_CLIENT_ID` overrides the audited public default.
    #[must_use]
    pub fn new() -> Self {
        let client_id = std::env::var(CLAUDE_OAUTH_CLIENT_ID_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| CLAUDE_OAUTH_CLIENT_ID.to_owned());
        Self {
            token_endpoint: CLAUDE_OAUTH_TOKEN_ENDPOINT.to_owned(),
            client_id,
            request_timeout: CLAUDE_OAUTH_REQUEST_TIMEOUT,
        }
    }

    /// Creates a login flow with an explicit public client identifier.
    #[must_use]
    pub fn with_client_id(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            ..Self::new()
        }
    }

    #[cfg(test)]
    fn new_for_test(token_endpoint: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            token_endpoint: token_endpoint.into(),
            client_id: client_id.into(),
            request_timeout: CLAUDE_OAUTH_REQUEST_TIMEOUT,
        }
    }
}

impl Default for ClaudeOAuthLoginFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountLoginFlow for ClaudeOAuthLoginFlow {
    type PendingLogin = PendingClaudeOAuthLogin;

    fn begin_login(&self) -> Result<Self::PendingLogin, LoginFlowError> {
        let mut state_bytes = [0_u8; 32];
        let mut verifier_bytes = [0_u8; 32];
        getrandom::fill(&mut state_bytes).map_err(|_| LoginFlowError::SecureRandomUnavailable)?;
        getrandom::fill(&mut verifier_bytes)
            .map_err(|_| LoginFlowError::SecureRandomUnavailable)?;
        Ok(PendingClaudeOAuthLogin {
            state: URL_SAFE_NO_PAD.encode(state_bytes),
            code_verifier: URL_SAFE_NO_PAD.encode(verifier_bytes),
        })
    }

    fn authorization_url(&self, pending: &Self::PendingLogin) -> Result<String, LoginFlowError> {
        let mut url = reqwest::Url::parse(CLAUDE_OAUTH_AUTHORIZE_ENDPOINT)
            .map_err(|_| LoginFlowError::AuthorizationRequestFailed)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(pending.code_verifier.as_bytes()));
        url.query_pairs_mut()
            .append_pair("code", "true")
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", CLAUDE_OAUTH_REDIRECT_URI)
            .append_pair("client_id", &self.client_id)
            .append_pair("scope", CLAUDE_OAUTH_SCOPE)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &pending.state);
        Ok(url.into())
    }

    fn finish_login(
        &self,
        pending: Self::PendingLogin,
        pasted_callback: &str,
    ) -> Result<CredentialBundle, LoginFlowError> {
        let callback = pasted_callback.trim();
        let (authorization_code, returned_state) = callback
            .split_once('#')
            .filter(|(code, state)| {
                !code.trim().is_empty() && !state.trim().is_empty() && !state.contains('#')
            })
            .ok_or(LoginFlowError::InvalidCallback)?;
        if returned_state != pending.state {
            return Err(LoginFlowError::StateMismatch);
        }

        // Request and response shape verified against claude-oss-proxy-source-audit.md §1.1–1.2:
        // Better CCflare oauth.ts L50–106 and provider.ts L218–250, plus CRS oauthHelper.js
        // L156–207. The redirect stays Anthropic-hosted; Router never opens a local callback.
        let request_body = ClaudeAuthorizationCodeRequest {
            grant_type: "authorization_code",
            code: authorization_code,
            state: returned_state,
            client_id: &self.client_id,
            redirect_uri: CLAUDE_OAUTH_REDIRECT_URI,
            code_verifier: &pending.code_verifier,
        };
        let request_bytes = serde_json::to_vec(&request_body)
            .map_err(|_| LoginFlowError::AuthorizationRequestFailed)?;
        let response = Client::builder()
            .timeout(self.request_timeout)
            .build()
            .map_err(|_| LoginFlowError::AuthorizationRequestFailed)?
            .post(&self.token_endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_bytes)
            .send()
            .map_err(|_| LoginFlowError::AuthorizationRequestFailed)?;
        if !response.status().is_success() {
            return Err(LoginFlowError::ProviderRefused);
        }
        let response_body = response
            .text()
            .map_err(|_| LoginFlowError::InvalidTokenResponse)?;
        let token_response = serde_json::from_str::<ClaudeOAuthTokenResponse>(&response_body)
            .map_err(|_| LoginFlowError::InvalidTokenResponse)?;
        let refresh_token = token_response
            .refresh_token
            .filter(|token| !token.trim().is_empty())
            .ok_or(LoginFlowError::InvalidTokenResponse)?;
        let expires_at = token_expiry_unix_seconds(token_response.expires_in)
            .ok_or(LoginFlowError::InvalidTokenResponse)?;
        CredentialBundle::new_claude(
            SecretString::new(token_response.access_token),
            SecretString::new(refresh_token),
            expires_at,
        )
        .map_err(|_| LoginFlowError::InvalidTokenResponse)
    }
}

/// Refreshes Claude subscription tokens at Anthropic's platform token endpoint.
#[derive(Clone, Debug)]
pub struct ClaudeOAuthRefreshClient {
    token_endpoint: String,
    client_id: String,
    request_timeout: Duration,
}

impl ClaudeOAuthRefreshClient {
    /// Creates a refresh client using Claude Code's public OAuth client identifier.
    #[must_use]
    pub fn new() -> Self {
        let login_flow = ClaudeOAuthLoginFlow::new();
        Self {
            token_endpoint: login_flow.token_endpoint,
            client_id: login_flow.client_id,
            request_timeout: CLAUDE_OAUTH_REQUEST_TIMEOUT,
        }
    }

    /// Refreshes one account and returns replacement credentials only on known success.
    pub fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<CredentialBundle, CredentialRefreshFailure> {
        // Wire shape and omitted-token rotation fallback verified against
        // claude-oss-proxy-source-audit.md §1.2 (Better CCflare provider.ts L218–250).
        let request_body = ClaudeRefreshTokenRequest {
            grant_type: "refresh_token",
            refresh_token: refresh_token.expose_secret(),
            client_id: &self.client_id,
        };
        let request_bytes = serde_json::to_vec(&request_body).map_err(|_| {
            CredentialRefreshFailure::confirmed_unspent(
                CredentialFailureClass::TransportUnspent,
                None,
            )
        })?;
        let client = Client::builder()
            .timeout(self.request_timeout)
            .build()
            .map_err(|_| {
                CredentialRefreshFailure::confirmed_unspent(
                    CredentialFailureClass::TransportUnspent,
                    None,
                )
            })?;
        let response = client
            .post(&self.token_endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_bytes)
            .send()
            .map_err(|error| {
                if error.is_connect() {
                    CredentialRefreshFailure::confirmed_unspent(
                        CredentialFailureClass::TransportUnspent,
                        None,
                    )
                } else {
                    CredentialRefreshFailure::ambiguous(
                        CredentialFailureClass::ProviderOutcomeAmbiguous,
                    )
                }
            })?;
        let retry_after_seconds = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(CredentialRefreshFailure::confirmed_unspent(
                CredentialFailureClass::RateLimited,
                retry_after_seconds,
            ));
        }
        let status = response.status();
        let response_body = response.text().map_err(|_| {
            CredentialRefreshFailure::ambiguous(CredentialFailureClass::ProviderOutcomeAmbiguous)
        })?;
        if !status.is_success() {
            let error_response =
                serde_json::from_str::<ClaudeOAuthErrorResponse>(&response_body).ok();
            let failure_class = if error_response.as_ref().is_some_and(|error| {
                matches!(
                    error.error.as_str(),
                    "invalid_grant" | "invalid_refresh_token"
                )
            }) {
                CredentialFailureClass::ProviderRejected
            } else {
                CredentialFailureClass::ProviderOutcomeAmbiguous
            };
            return Err(CredentialRefreshFailure::ambiguous(failure_class));
        }
        let token_response = serde_json::from_str::<ClaudeOAuthTokenResponse>(&response_body)
            .map_err(|_| {
                CredentialRefreshFailure::ambiguous(CredentialFailureClass::MalformedResponse)
            })?;
        let rotated_refresh_token = token_response
            .refresh_token
            .filter(|token| !token.trim().is_empty())
            .unwrap_or_else(|| refresh_token.expose_secret().to_owned());
        let expires_at = token_expiry_unix_seconds(token_response.expires_in).ok_or_else(|| {
            CredentialRefreshFailure::ambiguous(CredentialFailureClass::MalformedResponse)
        })?;
        CredentialBundle::new_claude(
            SecretString::new(token_response.access_token),
            SecretString::new(rotated_refresh_token),
            expires_at,
        )
        .map_err(|_| CredentialRefreshFailure::ambiguous(CredentialFailureClass::MalformedResponse))
    }
}

impl Default for ClaudeOAuthRefreshClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialRefreshClient for ClaudeOAuthRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<
        codex_router_secret_store::account_tokens::AccountCredentialBundle,
        CredentialRefreshFailure,
    > {
        Err(CredentialRefreshFailure::confirmed_unspent(
            CredentialFailureClass::LocalPersistence,
            None,
        ))
    }

    fn refresh_provider_credentials(
        &self,
        provider: Provider,
        account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<CredentialBundle, CredentialRefreshFailure> {
        if provider != Provider::Claude {
            return Err(CredentialRefreshFailure::ambiguous(
                CredentialFailureClass::ProviderOutcomeAmbiguous,
            ));
        }
        ClaudeOAuthRefreshClient::refresh_credentials(self, account_id, refresh_token)
    }
}

impl ClaudeOAuthRefreshClient {
    #[cfg(test)]
    fn new_with_endpoint_for_test(
        token_endpoint: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            token_endpoint: token_endpoint.into(),
            client_id: client_id.into(),
            request_timeout: Duration::from_secs(5),
        }
    }

    #[cfg(test)]
    fn new_with_endpoint_and_timeout_for_test(
        token_endpoint: impl Into<String>,
        client_id: impl Into<String>,
        request_timeout: Duration,
    ) -> Self {
        Self {
            token_endpoint: token_endpoint.into(),
            client_id: client_id.into(),
            request_timeout,
        }
    }
}

#[derive(Serialize)]
struct ClaudeAuthorizationCodeRequest<'a> {
    grant_type: &'static str,
    code: &'a str,
    state: &'a str,
    client_id: &'a str,
    redirect_uri: &'static str,
    code_verifier: &'a str,
}

#[derive(Serialize)]
struct ClaudeRefreshTokenRequest<'a> {
    grant_type: &'static str,
    refresh_token: &'a str,
    client_id: &'a str,
}

#[derive(Deserialize)]
struct ClaudeOAuthTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Deserialize)]
struct ClaudeOAuthErrorResponse {
    error: String,
}

fn token_expiry_unix_seconds(expires_in_seconds: u64) -> Option<u64> {
    if expires_in_seconds == 0 {
        return None;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    now.checked_add(expires_in_seconds)
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::net::TcpStream;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use codex_router_core::ids::AccountId;
    use codex_router_core::provider::Provider;
    use codex_router_core::redaction::SecretString;
    use serde_json::Value;
    use sha2::Digest;
    use sha2::Sha256;

    use crate::claude_oauth::AccountLoginFlow;
    use crate::claude_oauth::CLAUDE_OAUTH_CLIENT_ID;
    use crate::claude_oauth::CLAUDE_OAUTH_REDIRECT_URI;
    use crate::claude_oauth::CLAUDE_OAUTH_SCOPE;
    use crate::claude_oauth::ClaudeOAuthLoginFlow;
    use crate::claude_oauth::ClaudeOAuthRefreshClient;
    use crate::claude_oauth::CredentialFailureClass;
    use crate::claude_oauth::CredentialRefreshFailure;
    use crate::claude_oauth::LoginFlowError;

    #[test]
    fn authorization_url_uses_hosted_callback_public_client_and_s256_pkce() {
        let flow = ClaudeOAuthLoginFlow::new_for_test(
            format!("http://127.0.0.1:{}/oauth/token", unused_loopback_port()),
            "test-claude-client",
        );
        let pending = flow
            .begin_login()
            .expect("OAuth login start should generate PKCE material");
        let authorization_url = flow
            .authorization_url(&pending)
            .expect("authorization URL should be valid");
        let url =
            reqwest::Url::parse(&authorization_url).expect("authorization URL should be valid");
        let parameters = url
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            url.as_str().split('?').next(),
            Some("https://claude.ai/oauth/authorize")
        );
        assert_eq!(parameters.get("code").map(String::as_str), Some("true"));
        assert_eq!(
            parameters.get("response_type").map(String::as_str),
            Some("code")
        );
        assert_eq!(
            parameters.get("redirect_uri").map(String::as_str),
            Some(CLAUDE_OAUTH_REDIRECT_URI)
        );
        assert_eq!(
            parameters.get("client_id").map(String::as_str),
            Some("test-claude-client")
        );
        assert_eq!(
            parameters.get("scope").map(String::as_str),
            Some(CLAUDE_OAUTH_SCOPE)
        );
        assert_eq!(
            parameters.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert!(
            parameters
                .get("code_challenge")
                .is_some_and(|challenge| !challenge.is_empty())
        );
        assert!(
            parameters
                .get("state")
                .is_some_and(|state| !state.is_empty())
        );
        assert_eq!(
            url.query_pairs()
                .find(|(name, _)| name == "client_id")
                .map(|(_, value)| value.into_owned()),
            Some("test-claude-client".to_owned())
        );
        assert!(!format!("{pending:?}").contains(pending.code_verifier.as_str()));
        assert_eq!(
            CLAUDE_OAUTH_CLIENT_ID,
            "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
        );
    }

    #[test]
    fn authorization_code_exchange_checks_state_and_pkce_and_activates_tokens_in_memory() {
        let (endpoint, request_receiver, server_thread) = one_response_server(
            200,
            r#"{"access_token":"claude-access-canary","refresh_token":"claude-refresh-canary","expires_in":3600}"#,
        );
        let flow = ClaudeOAuthLoginFlow::new_for_test(endpoint, "test-claude-client");
        let pending = flow
            .begin_login()
            .expect("OAuth login start should generate PKCE material");
        let authorization_url = flow
            .authorization_url(&pending)
            .expect("authorization URL should be valid");
        let authorization_url =
            reqwest::Url::parse(&authorization_url).expect("authorization URL should be valid");
        let parameters = authorization_url
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        let state = parameters.get("state").expect("state should be present");
        let expected_challenge = parameters
            .get("code_challenge")
            .expect("PKCE challenge should be present");
        let pasted_code = format!("authorization-code-canary#{state}");

        let bundle = flow
            .finish_login(pending, &pasted_code)
            .expect("valid hosted callback code should exchange successfully");
        let request = request_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("fake OAuth server should observe one token exchange");
        let request_body = request_body_json(&request);
        let verifier = request_body
            .get("code_verifier")
            .and_then(Value::as_str)
            .expect("exchange should carry its PKCE verifier");
        let verifier_hash = Sha256::digest(verifier.as_bytes());

        assert_eq!(http_request_path(&request), "/oauth/token");
        assert_eq!(
            request_body.get("grant_type").and_then(Value::as_str),
            Some("authorization_code")
        );
        assert_eq!(
            request_body.get("code").and_then(Value::as_str),
            Some("authorization-code-canary")
        );
        assert_eq!(
            request_body.get("state").and_then(Value::as_str),
            Some(state.as_str())
        );
        assert_eq!(
            request_body.get("redirect_uri").and_then(Value::as_str),
            Some(CLAUDE_OAUTH_REDIRECT_URI)
        );
        assert_eq!(
            request_body.get("client_id").and_then(Value::as_str),
            Some("test-claude-client")
        );
        assert_eq!(URL_SAFE_NO_PAD.encode(verifier_hash), *expected_challenge);
        assert_eq!(bundle.provider(), Provider::Claude);
        assert_eq!(
            bundle.access_token().expose_secret(),
            "claude-access-canary"
        );
        assert_eq!(
            bundle.refresh_token().map(SecretString::expose_secret),
            Some("claude-refresh-canary")
        );
        assert!(bundle.expires_unix_seconds().is_some());
        assert!(!format!("{bundle:?}").contains("claude-access-canary"));
        server_thread
            .join()
            .expect("fake OAuth server should finish");
    }

    #[test]
    fn callback_state_mismatch_fails_before_token_endpoint_egress() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        let endpoint = format!(
            "http://{}/oauth/token",
            listener.local_addr().expect("listener address")
        );
        listener
            .set_nonblocking(true)
            .expect("listener should support nonblocking observation");
        let flow = ClaudeOAuthLoginFlow::new_for_test(endpoint, "test-claude-client");
        let pending = flow
            .begin_login()
            .expect("OAuth login start should generate PKCE material");

        assert_eq!(
            flow.finish_login(pending, "authorization-code-canary#wrong-state")
                .err(),
            Some(LoginFlowError::StateMismatch)
        );
        assert!(
            matches!(
                listener.accept(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
            ),
            "a mismatched callback state must not reach the token endpoint"
        );
    }

    #[test]
    fn refresh_rotation_uses_replacement_or_preserves_old_token() {
        for (response_body, expected_refresh) in [
            (
                r#"{"access_token":"rotated-access","refresh_token":"rotated-refresh","expires_in":3600}"#,
                "rotated-refresh",
            ),
            (
                r#"{"access_token":"rotated-access","expires_in":3600}"#,
                "old-refresh-canary",
            ),
        ] {
            let (endpoint, request_receiver, server_thread) =
                one_response_server(200, response_body);
            let client = ClaudeOAuthRefreshClient::new_with_endpoint_for_test(
                endpoint,
                "test-claude-client",
            );
            let account_id =
                AccountId::new("acct_claude_refresh").expect("account id should be valid");
            let bundle = client
                .refresh_credentials(&account_id, &SecretString::new("old-refresh-canary"))
                .expect("refresh response should produce a Claude bundle");
            let request = request_receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("fake OAuth server should observe refresh request");
            let request_body = request_body_json(&request);

            assert_eq!(
                request_body.get("grant_type").and_then(Value::as_str),
                Some("refresh_token")
            );
            assert_eq!(
                request_body.get("client_id").and_then(Value::as_str),
                Some("test-claude-client")
            );
            assert_eq!(
                request_body.get("refresh_token").and_then(Value::as_str),
                Some("old-refresh-canary")
            );
            assert_eq!(
                bundle.refresh_token().map(SecretString::expose_secret),
                Some(expected_refresh)
            );
            server_thread
                .join()
                .expect("fake OAuth server should finish");
        }
    }

    #[test]
    fn definitive_refresh_refusal_requires_login() {
        let (endpoint, _request_receiver, server_thread) =
            one_response_server(400, r#"{"error":"invalid_grant"}"#);
        let client =
            ClaudeOAuthRefreshClient::new_with_endpoint_for_test(endpoint, "test-claude-client");
        let account_id = AccountId::new("acct_claude_refused").expect("account id should be valid");

        assert_eq!(
            client.refresh_credentials(&account_id, &SecretString::new("old-refresh-canary")),
            Err(CredentialRefreshFailure::ambiguous(
                CredentialFailureClass::ProviderRejected
            ))
        );
        server_thread
            .join()
            .expect("fake OAuth server should finish");
    }

    #[test]
    fn refresh_timeout_is_ambiguous_and_never_returns_the_old_token() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        let endpoint = format!(
            "http://{}/oauth/token",
            listener.local_addr().expect("listener address")
        );
        let (request_sender, request_receiver) = mpsc::channel();
        let server_thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept token request");
            let request = read_http_request(&mut stream);
            request_sender
                .send(request)
                .expect("test should receive the request");
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut byte = [0_u8; 1];
            loop {
                match stream.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        let client = ClaudeOAuthRefreshClient::new_with_endpoint_and_timeout_for_test(
            endpoint,
            "test-claude-client",
            Duration::from_millis(100),
        );
        let account_id = AccountId::new("acct_claude_timeout").expect("account id should be valid");

        assert_eq!(
            client.refresh_credentials(
                &account_id,
                &SecretString::new("possibly-consumed-refresh-canary")
            ),
            Err(CredentialRefreshFailure::ambiguous(
                CredentialFailureClass::ProviderOutcomeAmbiguous
            ))
        );
        let request = request_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("fake server should receive the refresh before timing out");
        assert_eq!(
            request_body_json(&request)
                .get("refresh_token")
                .and_then(Value::as_str),
            Some("possibly-consumed-refresh-canary")
        );
        server_thread
            .join()
            .expect("fake OAuth server should observe the timeout close");
    }

    fn one_response_server(
        status: u16,
        body: &'static str,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        let endpoint = format!(
            "http://{}/oauth/token",
            listener.local_addr().expect("listener address")
        );
        let (request_sender, request_receiver) = mpsc::channel();
        let server_thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept token request");
            let request = read_http_request(&mut stream);
            request_sender
                .send(request)
                .expect("test should receive the request");
            let reason = if status == 200 { "OK" } else { "Bad Request" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write token response");
        });
        (endpoint, request_receiver, server_thread)
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut request_bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let read_count = stream.read(&mut buffer).expect("read request bytes");
            assert!(read_count > 0, "request should include HTTP headers");
            request_bytes.extend_from_slice(&buffer[..read_count]);
            if let Some(header_end) = request_bytes
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
            {
                break header_end + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request_bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while request_bytes.len().saturating_sub(header_end) < content_length {
            let read_count = stream.read(&mut buffer).expect("read request body");
            assert!(read_count > 0, "request body should match content length");
            request_bytes.extend_from_slice(&buffer[..read_count]);
        }
        String::from_utf8(request_bytes).expect("HTTP request should be UTF-8")
    }

    fn request_body_json(request: &str) -> Value {
        let (_, body) = request
            .split_once("\r\n\r\n")
            .expect("HTTP request should separate headers from body");
        serde_json::from_str(body).expect("OAuth body should be JSON")
    }

    fn http_request_path(request: &str) -> &str {
        request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .expect("HTTP request line should include the path")
    }

    fn unused_loopback_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        listener.local_addr().expect("listener address").port()
    }
}
