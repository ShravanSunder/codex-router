//! Router-native OpenAI OAuth device-code login.

use std::fmt;
use std::time::Duration;
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_router_core::redaction::SecretString;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

const OPENAI_OAUTH_ISSUER: &str = "https://auth.openai.com";
pub(crate) const OPENAI_OAUTH_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
pub(crate) const OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_DEVICE_CODE_MAX_WAIT: Duration = Duration::from_secs(15 * 60);

/// A user-visible device code and the opaque issuer value required to poll it.
pub struct OpenAiDeviceCode {
    verification_url: String,
    user_code: String,
    device_auth_id: SecretString,
    poll_interval: Duration,
}

impl OpenAiDeviceCode {
    /// Returns the URL the user opens to approve the login.
    #[must_use]
    pub fn verification_url(&self) -> &str {
        &self.verification_url
    }

    /// Returns the one-time code the user enters at the verification URL.
    #[must_use]
    pub fn user_code(&self) -> &str {
        &self.user_code
    }
}

impl fmt::Debug for OpenAiDeviceCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiDeviceCode")
            .field("verification_url", &"[REDACTED]")
            .field("user_code", &"[REDACTED]")
            .field("device_auth_id", &self.device_auth_id)
            .field("poll_interval", &self.poll_interval)
            .finish()
    }
}

/// A validated non-empty ChatGPT account id read from the OpenAI id-token claims.
#[derive(Clone, Eq, PartialEq)]
pub struct ChatGptAccountId(String);

impl ChatGptAccountId {
    fn new(value: String) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }

        Some(Self(value.to_owned()))
    }

    /// Returns the provider account id without changing its wire representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ChatGptAccountId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ChatGptAccountId([REDACTED])")
    }
}

/// OpenAI OAuth credentials returned after the issuer approves the device login.
pub struct OpenAiOAuthLoginTokens {
    access_token: SecretString,
    refresh_token: SecretString,
    chatgpt_account_id: Option<ChatGptAccountId>,
}

impl OpenAiOAuthLoginTokens {
    /// Returns the access token for immediate encrypted activation.
    #[must_use]
    pub fn access_token(&self) -> &SecretString {
        &self.access_token
    }

    /// Returns the refresh token for encrypted activation.
    #[must_use]
    pub fn refresh_token(&self) -> &SecretString {
        &self.refresh_token
    }

    /// Returns the optional ChatGPT account id claim used by Router today.
    #[must_use]
    pub fn chatgpt_account_id(&self) -> Option<&ChatGptAccountId> {
        self.chatgpt_account_id.as_ref()
    }
}

impl fmt::Debug for OpenAiOAuthLoginTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiOAuthLoginTokens")
            .field("access_token", &self.access_token)
            .field("refresh_token", &self.refresh_token)
            .field(
                "chatgpt_account_id",
                &self.chatgpt_account_id.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// Failure while requesting or completing the OpenAI device-code login.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum OpenAiOAuthDeviceLoginError {
    /// The issuer did not enable device-code login for this endpoint.
    #[error("OpenAI device-code login is not enabled by the issuer")]
    DeviceCodeUnavailable,
    /// The user-code endpoint failed before returning a device code.
    #[error("OpenAI user-code request failed")]
    UserCodeTransportFailure,
    /// The issuer rejected the user-code request.
    #[error("OpenAI user-code request was rejected (HTTP {status})")]
    UserCodeRejected { status: u16 },
    /// The issuer returned an invalid user-code response.
    #[error("OpenAI returned an invalid user-code response")]
    InvalidUserCodeResponse,
    /// Polling the device authorization endpoint failed in transport.
    #[error("OpenAI device authorization poll failed")]
    PollTransportFailure,
    /// The issuer rejected the device authorization poll.
    #[error("OpenAI device authorization was rejected (HTTP {status})")]
    PollRejected { status: u16 },
    /// The issuer returned invalid poll data or PKCE fields.
    #[error("OpenAI returned an invalid device authorization response")]
    InvalidPollResponse,
    /// The device authorization remained pending until its 15-minute limit.
    #[error("OpenAI device authorization expired before approval")]
    Expired,
    /// The authorization-code token exchange failed in transport.
    #[error("OpenAI authorization-code exchange failed")]
    TokenExchangeTransportFailure,
    /// The issuer rejected the authorization-code token exchange.
    #[error("OpenAI authorization-code exchange was rejected (HTTP {status})")]
    TokenExchangeRejected { status: u16 },
    /// The issuer returned an invalid token exchange response.
    #[error("OpenAI returned an invalid token exchange response")]
    InvalidTokenExchangeResponse,
    /// The owner cancelled the device-code login.
    #[error("OpenAI device-code login was cancelled")]
    Cancelled,
}

/// Performs the OpenAI device-code flow without creating local token files.
#[derive(Clone)]
pub struct OpenAiOAuthDeviceLoginClient {
    accounts_base_url: String,
    token_endpoint: String,
    redirect_uri: String,
    verification_url: String,
    http_client: reqwest::Client,
    max_poll_duration: Duration,
}

impl OpenAiOAuthDeviceLoginClient {
    /// Creates a client using the production OpenAI issuer and token endpoint.
    #[must_use]
    pub fn new() -> Self {
        Self::for_issuer(
            OPENAI_OAUTH_ISSUER,
            OPENAI_OAUTH_TOKEN_ENDPOINT,
            OPENAI_DEVICE_CODE_MAX_WAIT,
        )
    }

    /// Creates a client for a fake issuer in tests.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_test_issuer(
        issuer_base_url: impl Into<String>,
        max_poll_duration: Duration,
    ) -> Self {
        let issuer_base_url = issuer_base_url.into();
        let issuer_base_url = issuer_base_url.trim_end_matches('/');
        let token_endpoint = format!("{issuer_base_url}/oauth/token");
        Self::for_issuer(issuer_base_url, &token_endpoint, max_poll_duration)
    }

    fn for_issuer(
        issuer_base_url: &str,
        token_endpoint: &str,
        max_poll_duration: Duration,
    ) -> Self {
        let issuer_base_url = issuer_base_url.trim_end_matches('/').to_owned();
        let accounts_base_url = format!("{issuer_base_url}/api/accounts");
        let redirect_uri = format!("{issuer_base_url}/deviceauth/callback");
        let verification_url = format!("{issuer_base_url}/codex/device");

        Self {
            accounts_base_url,
            token_endpoint: token_endpoint.to_owned(),
            redirect_uri,
            verification_url,
            http_client: reqwest::Client::new(),
            max_poll_duration,
        }
    }

    /// Requests a one-time user code from OpenAI.
    pub async fn request_user_code(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<OpenAiDeviceCode, OpenAiOAuthDeviceLoginError> {
        let request = UserCodeRequest {
            client_id: OPENAI_OAUTH_CLIENT_ID,
        };
        let request_body = serde_json::to_vec(&request)
            .map_err(|_| OpenAiOAuthDeviceLoginError::InvalidUserCodeResponse)?;
        let response = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(OpenAiOAuthDeviceLoginError::Cancelled);
            }
            response = self.http_client
                .post(format!("{}/deviceauth/usercode", self.accounts_base_url))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(request_body)
                .send() => response.map_err(|_| OpenAiOAuthDeviceLoginError::UserCodeTransportFailure)?,
        };

        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Err(OpenAiOAuthDeviceLoginError::DeviceCodeUnavailable);
        }
        if !status.is_success() {
            return Err(OpenAiOAuthDeviceLoginError::UserCodeRejected {
                status: status.as_u16(),
            });
        }

        let response_body = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(OpenAiOAuthDeviceLoginError::Cancelled);
            }
            response_body = response.text() => {
                response_body.map_err(|_| OpenAiOAuthDeviceLoginError::InvalidUserCodeResponse)?
            }
        };
        let response: UserCodeResponse = serde_json::from_str(&response_body)
            .map_err(|_| OpenAiOAuthDeviceLoginError::InvalidUserCodeResponse)?;
        if response.device_auth_id.trim().is_empty() || response.user_code.trim().is_empty() {
            return Err(OpenAiOAuthDeviceLoginError::InvalidUserCodeResponse);
        }

        Ok(OpenAiDeviceCode {
            verification_url: self.verification_url.clone(),
            user_code: response.user_code,
            device_auth_id: SecretString::new(response.device_auth_id),
            poll_interval: Duration::from_secs(response.interval),
        })
    }

    /// Polls until approval, timeout, denial or cancellation, then exchanges the PKCE verifier.
    pub async fn complete_device_code_login(
        &self,
        device_code: &OpenAiDeviceCode,
        cancellation: &CancellationToken,
    ) -> Result<OpenAiOAuthLoginTokens, OpenAiOAuthDeviceLoginError> {
        let authorization = self
            .poll_for_authorization(device_code, cancellation)
            .await?;
        let request = AuthorizationCodeExchangeRequest {
            grant_type: "authorization_code",
            code: &authorization.authorization_code,
            redirect_uri: &self.redirect_uri,
            client_id: OPENAI_OAUTH_CLIENT_ID,
            code_verifier: &authorization.code_verifier,
        };
        let response = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(OpenAiOAuthDeviceLoginError::Cancelled);
            }
            response = self.http_client
                .post(&self.token_endpoint)
                .form(&request)
                .send() => response.map_err(|_| OpenAiOAuthDeviceLoginError::TokenExchangeTransportFailure)?,
        };
        let status = response.status();
        if !status.is_success() {
            return Err(OpenAiOAuthDeviceLoginError::TokenExchangeRejected {
                status: status.as_u16(),
            });
        }

        let response_body = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(OpenAiOAuthDeviceLoginError::Cancelled);
            }
            response_body = response.text() => {
                response_body.map_err(|_| OpenAiOAuthDeviceLoginError::InvalidTokenExchangeResponse)?
            }
        };
        let response: TokenExchangeResponse = serde_json::from_str(&response_body)
            .map_err(|_| OpenAiOAuthDeviceLoginError::InvalidTokenExchangeResponse)?;
        let access_token = response.access_token.trim();
        let refresh_token = response.refresh_token.trim();
        if access_token.is_empty()
            || refresh_token.is_empty()
            || response.id_token.trim().is_empty()
        {
            return Err(OpenAiOAuthDeviceLoginError::InvalidTokenExchangeResponse);
        }

        Ok(OpenAiOAuthLoginTokens {
            access_token: SecretString::new(access_token),
            refresh_token: SecretString::new(refresh_token),
            chatgpt_account_id: chatgpt_account_id_from_id_token(&response.id_token),
        })
    }

    async fn poll_for_authorization(
        &self,
        device_code: &OpenAiDeviceCode,
        cancellation: &CancellationToken,
    ) -> Result<AuthorizationCodeResponse, OpenAiOAuthDeviceLoginError> {
        let started_at = Instant::now();
        loop {
            let request = TokenPollRequest {
                device_auth_id: device_code.device_auth_id.expose_secret(),
                user_code: &device_code.user_code,
            };
            let request_body = serde_json::to_vec(&request)
                .map_err(|_| OpenAiOAuthDeviceLoginError::InvalidPollResponse)?;
            let response = tokio::select! {
                () = cancellation.cancelled() => {
                    return Err(OpenAiOAuthDeviceLoginError::Cancelled);
                }
                response = self.http_client
                    .post(format!("{}/deviceauth/token", self.accounts_base_url))
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(request_body)
                    .send() => response.map_err(|_| OpenAiOAuthDeviceLoginError::PollTransportFailure)?,
            };

            let status = response.status();
            if status.is_success() {
                let response_body = tokio::select! {
                    () = cancellation.cancelled() => {
                        return Err(OpenAiOAuthDeviceLoginError::Cancelled);
                    }
                    response_body = response.text() => {
                        response_body.map_err(|_| OpenAiOAuthDeviceLoginError::InvalidPollResponse)?
                    }
                };
                let authorization: AuthorizationCodeResponse = serde_json::from_str(&response_body)
                    .map_err(|_| OpenAiOAuthDeviceLoginError::InvalidPollResponse)?;
                if authorization.authorization_code.trim().is_empty()
                    || authorization.code_challenge.trim().is_empty()
                    || authorization.code_verifier.trim().is_empty()
                {
                    return Err(OpenAiOAuthDeviceLoginError::InvalidPollResponse);
                }
                return Ok(authorization);
            }

            if status != StatusCode::FORBIDDEN && status != StatusCode::NOT_FOUND {
                return Err(OpenAiOAuthDeviceLoginError::PollRejected {
                    status: status.as_u16(),
                });
            }

            let elapsed = started_at.elapsed();
            if elapsed >= self.max_poll_duration {
                return Err(OpenAiOAuthDeviceLoginError::Expired);
            }
            let wait_duration = device_code
                .poll_interval
                .min(self.max_poll_duration.saturating_sub(elapsed));
            tokio::select! {
                () = cancellation.cancelled() => {
                    return Err(OpenAiOAuthDeviceLoginError::Cancelled);
                }
                () = tokio::time::sleep(wait_duration) => {}
            }
        }
    }
}

impl Default for OpenAiOAuthDeviceLoginClient {
    fn default() -> Self {
        Self::new()
    }
}

fn chatgpt_account_id_from_id_token(id_token: &str) -> Option<ChatGptAccountId> {
    let payload_segment = id_token.split('.').nth(1)?;
    let payload = URL_SAFE_NO_PAD.decode(payload_segment).ok()?;
    let claims: IdTokenClaims = serde_json::from_slice(&payload).ok()?;
    ChatGptAccountId::new(claims.openai_auth?.chatgpt_account_id?)
}

#[derive(Deserialize)]
struct UserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "user_code", alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "deserialize_poll_interval")]
    interval: u64,
}

#[derive(Serialize)]
struct UserCodeRequest {
    client_id: &'static str,
}

#[derive(Serialize)]
struct TokenPollRequest<'a> {
    device_auth_id: &'a str,
    user_code: &'a str,
}

#[derive(Deserialize)]
struct AuthorizationCodeResponse {
    authorization_code: String,
    #[serde(rename = "code_challenge")]
    code_challenge: String,
    code_verifier: String,
}

#[derive(Serialize)]
struct AuthorizationCodeExchangeRequest<'a> {
    grant_type: &'static str,
    code: &'a str,
    redirect_uri: &'a str,
    client_id: &'static str,
    code_verifier: &'a str,
}

#[derive(Deserialize)]
struct TokenExchangeResponse {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

#[derive(Deserialize)]
struct IdTokenClaims {
    #[serde(rename = "https://api.openai.com/auth")]
    openai_auth: Option<OpenAiIdTokenAuthClaims>,
}

#[derive(Deserialize)]
struct OpenAiIdTokenAuthClaims {
    chatgpt_account_id: Option<String>,
}

fn deserialize_poll_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let interval = String::deserialize(deserializer)?;
    interval
        .trim()
        .parse::<u64>()
        .map_err(serde::de::Error::custom)
}

#[cfg(test)]
#[path = "tests/openai_oauth_device_login_tests.rs"]
mod openai_oauth_device_login_tests;
