//! Secret-key conventions for upstream OpenAI account token material.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use serde::Deserialize;
use serde::Serialize;

use crate::model::SecretKey;
use crate::model::SecretStoreError;

/// Version of the router-owned bundled account credential payload.
pub const ACCOUNT_CREDENTIAL_BUNDLE_VERSION: u8 = 1;

/// Router-owned active credential bundle for one upstream OpenAI account.
#[derive(Clone, Eq, PartialEq)]
pub struct AccountCredentialBundle {
    access_token: SecretString,
    refresh_token: Option<SecretString>,
    expires_unix_seconds: Option<u64>,
    source: String,
    chatgpt_account_id: Option<String>,
}

impl AccountCredentialBundle {
    /// Creates a credential bundle imported from an existing Codex auth file.
    #[must_use]
    pub fn imported_codex_auth(
        access_token: impl Into<String>,
        refresh_token: Option<String>,
    ) -> Self {
        let access_token = access_token.into();
        let expires_unix_seconds = access_token_exp_unix_seconds(&access_token);
        Self {
            access_token: SecretString::new(access_token),
            refresh_token: refresh_token.map(SecretString::new),
            expires_unix_seconds,
            source: "codex_auth_json".to_owned(),
            chatgpt_account_id: None,
        }
    }

    /// Sets the access-token expiry hint.
    #[must_use]
    pub const fn with_expires_unix_seconds(mut self, expires_unix_seconds: u64) -> Self {
        self.expires_unix_seconds = Some(expires_unix_seconds);
        self
    }

    /// Returns the access token.
    #[must_use]
    pub const fn access_token(&self) -> &SecretString {
        &self.access_token
    }

    /// Returns the refresh token when present.
    #[must_use]
    pub const fn refresh_token(&self) -> Option<&SecretString> {
        self.refresh_token.as_ref()
    }

    /// Returns the token expiry hint when present.
    #[must_use]
    pub const fn expires_unix_seconds(&self) -> Option<u64> {
        self.expires_unix_seconds
    }

    /// Returns the source that produced this bundle.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the ChatGPT account id when present.
    #[must_use]
    pub fn chatgpt_account_id(&self) -> Option<&str> {
        self.chatgpt_account_id.as_deref()
    }

    /// Sets the ChatGPT account id.
    #[must_use]
    pub fn with_chatgpt_account_id(mut self, chatgpt_account_id: impl Into<String>) -> Self {
        let chatgpt_account_id = chatgpt_account_id.into();
        if !chatgpt_account_id.trim().is_empty() {
            self.chatgpt_account_id = Some(chatgpt_account_id);
        }
        self
    }

    /// Serializes the bundle into one secret-store payload.
    pub fn to_secret_string(&self) -> Result<SecretString, SecretStoreError> {
        let payload = AccountCredentialBundlePayload {
            version: ACCOUNT_CREDENTIAL_BUNDLE_VERSION,
            access_token: self.access_token.expose_secret(),
            refresh_token: self.refresh_token.as_ref().map(SecretString::expose_secret),
            expires_unix_seconds: self.expires_unix_seconds,
            source: &self.source,
            chatgpt_account_id: self.chatgpt_account_id.as_deref(),
        };
        serde_json::to_string(&payload)
            .map(SecretString::new)
            .map_err(secret_payload_error)
    }

    /// Parses a bundle from one secret-store payload.
    pub fn from_secret_string(secret: SecretString) -> Result<Self, SecretStoreError> {
        let payload: OwnedAccountCredentialBundlePayload =
            serde_json::from_str(secret.expose_secret()).map_err(secret_payload_error)?;
        if payload.version != ACCOUNT_CREDENTIAL_BUNDLE_VERSION {
            return Err(SecretStoreError::InvalidSecretPayload {
                message: format!(
                    "unsupported account credential bundle version {}",
                    payload.version
                ),
            });
        }
        if payload.access_token.trim().is_empty() {
            return Err(SecretStoreError::InvalidSecretPayload {
                message: "account credential bundle missing access token".to_owned(),
            });
        }

        Ok(Self {
            expires_unix_seconds: payload
                .expires_unix_seconds
                .or_else(|| access_token_exp_unix_seconds(&payload.access_token)),
            access_token: SecretString::new(payload.access_token),
            refresh_token: payload
                .refresh_token
                .filter(|token| !token.trim().is_empty())
                .map(SecretString::new),
            source: payload.source,
            chatgpt_account_id: payload
                .chatgpt_account_id
                .map(|account_id| account_id.trim().to_owned())
                .filter(|account_id| !account_id.is_empty()),
        })
    }
}

impl fmt::Debug for AccountCredentialBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountCredentialBundle")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_unix_seconds", &self.expires_unix_seconds)
            .field("source", &self.source)
            .field("chatgpt_account_id", &self.chatgpt_account_id)
            .finish()
    }
}

#[derive(Serialize)]
struct AccountCredentialBundlePayload<'a> {
    version: u8,
    access_token: &'a str,
    refresh_token: Option<&'a str>,
    expires_unix_seconds: Option<u64>,
    source: &'a str,
    chatgpt_account_id: Option<&'a str>,
}

#[derive(Deserialize)]
struct OwnedAccountCredentialBundlePayload {
    version: u8,
    access_token: String,
    refresh_token: Option<String>,
    expires_unix_seconds: Option<u64>,
    source: String,
    chatgpt_account_id: Option<String>,
}

/// Builds the secret key for an account's upstream OpenAI access token.
pub fn upstream_access_token_key(account_id: &AccountId) -> Result<SecretKey, SecretStoreError> {
    SecretKey::new(format!("openai_access_token.{}", account_id.as_str()))
}

/// Builds the secret key for an account's upstream OpenAI refresh token.
pub fn upstream_refresh_token_key(account_id: &AccountId) -> Result<SecretKey, SecretStoreError> {
    SecretKey::new(format!("openai_refresh_token.{}", account_id.as_str()))
}

/// Builds the secret key for one bundled account credential generation.
pub fn account_credential_bundle_key(
    account_id: &AccountId,
    generation: u64,
) -> Result<SecretKey, SecretStoreError> {
    SecretKey::new(format!(
        "openai_credential_bundle.{}.{}",
        account_id.as_str(),
        generation
    ))
}

fn secret_payload_error(error: impl std::fmt::Display) -> SecretStoreError {
    SecretStoreError::InvalidSecretPayload {
        message: error.to_string(),
    }
}

fn access_token_exp_unix_seconds(access_token: &str) -> Option<u64> {
    let payload_segment = access_token.split('.').nth(1)?;
    let payload = URL_SAFE_NO_PAD.decode(payload_segment).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    value.get("exp").and_then(serde_json::Value::as_u64)
}
