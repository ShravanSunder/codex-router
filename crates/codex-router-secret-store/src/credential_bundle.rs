//! Provider-discriminated pooled account credential bundles.

use std::fmt;

use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use serde::Deserialize;
use serde::Serialize;

use crate::account_tokens::AccountCredentialBundle;
use crate::model::SecretStoreError;

const CLAUDE_CREDENTIAL_BUNDLE_VERSION: u8 = 1;

/// The typed credential variants stored under provider-scoped generations.
#[derive(Clone, Eq, PartialEq)]
pub enum CredentialBundle {
    /// Existing OpenAI account bundle and its version-one serialized shape.
    OpenAi(AccountCredentialBundle),
    /// Claude subscription OAuth tokens.
    Claude {
        /// OAuth access token.
        access: SecretString,
        /// Rotating OAuth refresh token.
        refresh: SecretString,
        /// Unix expiry time in seconds.
        expires_at: u64,
    },
}

impl CredentialBundle {
    /// Validates and creates one Claude subscription credential bundle.
    pub fn new_claude(
        access: SecretString,
        refresh: SecretString,
        expires_at: u64,
    ) -> Result<Self, SecretStoreError> {
        if access.expose_secret().trim().is_empty() {
            return Err(invalid_bundle("Claude access token is empty"));
        }
        if refresh.expose_secret().trim().is_empty() {
            return Err(invalid_bundle("Claude refresh token is empty"));
        }
        if expires_at == 0 {
            return Err(invalid_bundle("Claude token expiry is invalid"));
        }

        Ok(Self::Claude {
            access,
            refresh,
            expires_at,
        })
    }

    /// Returns the provider discriminator carried by this bundle.
    #[must_use]
    pub const fn provider(&self) -> Provider {
        match self {
            Self::OpenAi(_) => Provider::Openai,
            Self::Claude { .. } => Provider::Claude,
        }
    }

    /// Returns the access token.
    #[must_use]
    pub const fn access_token(&self) -> &SecretString {
        match self {
            Self::OpenAi(bundle) => bundle.access_token(),
            Self::Claude { access, .. } => access,
        }
    }

    /// Returns the refresh token when the provider supplied renewable material.
    #[must_use]
    pub const fn refresh_token(&self) -> Option<&SecretString> {
        match self {
            Self::OpenAi(bundle) => bundle.refresh_token(),
            Self::Claude { refresh, .. } => Some(refresh),
        }
    }

    /// Returns the access token expiry when known.
    #[must_use]
    pub const fn expires_unix_seconds(&self) -> Option<u64> {
        match self {
            Self::OpenAi(bundle) => bundle.expires_unix_seconds(),
            Self::Claude { expires_at, .. } => Some(*expires_at),
        }
    }

    /// Returns the ChatGPT account id used by OpenAI backend requests.
    #[must_use]
    pub fn chatgpt_account_id(&self) -> Option<&str> {
        match self {
            Self::OpenAi(bundle) => bundle.chatgpt_account_id(),
            Self::Claude { .. } => None,
        }
    }

    /// Adds one ChatGPT account id to an OpenAI variant while leaving Claude data untouched.
    #[must_use]
    pub fn with_openai_chatgpt_account_id(mut self, chatgpt_account_id: impl Into<String>) -> Self {
        if let Self::OpenAi(bundle) = self {
            self = Self::OpenAi(bundle.with_chatgpt_account_id(chatgpt_account_id));
        }
        self
    }

    /// Serializes the provider variant into one secret-store payload.
    ///
    /// OpenAI keeps the exact payload shape already stored by PR2. Claude has
    /// a provider-tagged payload under its separate generation-key namespace.
    pub fn to_secret_string(&self) -> Result<SecretString, SecretStoreError> {
        match self {
            Self::OpenAi(bundle) => bundle.to_secret_string(),
            Self::Claude {
                access,
                refresh,
                expires_at,
            } => {
                if access.expose_secret().trim().is_empty() {
                    return Err(invalid_bundle("Claude access token is empty"));
                }
                if refresh.expose_secret().trim().is_empty() {
                    return Err(invalid_bundle("Claude refresh token is empty"));
                }
                if *expires_at == 0 {
                    return Err(invalid_bundle("Claude token expiry is invalid"));
                }

                let payload = ClaudeCredentialBundlePayload {
                    version: CLAUDE_CREDENTIAL_BUNDLE_VERSION,
                    provider: "claude",
                    access_token: access.expose_secret(),
                    refresh_token: refresh.expose_secret(),
                    expires_at: *expires_at,
                };
                serde_json::to_string(&payload)
                    .map(SecretString::new)
                    .map_err(secret_payload_error)
            }
        }
    }

    /// Decodes one payload using the provider already encoded in its secret key.
    pub fn from_secret_string(
        provider: Provider,
        secret: SecretString,
    ) -> Result<Self, SecretStoreError> {
        match provider {
            Provider::Openai => {
                AccountCredentialBundle::from_secret_string(secret).map(Self::OpenAi)
            }
            Provider::Claude => {
                let payload: OwnedClaudeCredentialBundlePayload =
                    serde_json::from_str(secret.expose_secret()).map_err(secret_payload_error)?;
                if payload.version != CLAUDE_CREDENTIAL_BUNDLE_VERSION {
                    return Err(invalid_bundle(
                        "unsupported Claude credential bundle version",
                    ));
                }
                if payload.provider != "claude" {
                    return Err(invalid_bundle("Claude credential bundle provider mismatch"));
                }
                Self::new_claude(
                    SecretString::new(payload.access_token),
                    SecretString::new(payload.refresh_token),
                    payload.expires_at,
                )
            }
        }
    }
}

impl From<AccountCredentialBundle> for CredentialBundle {
    fn from(bundle: AccountCredentialBundle) -> Self {
        Self::OpenAi(bundle)
    }
}

impl fmt::Debug for CredentialBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenAi(bundle) => formatter
                .debug_tuple("CredentialBundle::OpenAi")
                .field(bundle)
                .finish(),
            Self::Claude { expires_at, .. } => formatter
                .debug_struct("CredentialBundle::Claude")
                .field("access", &"[REDACTED]")
                .field("refresh", &"[REDACTED]")
                .field("expires_at", expires_at)
                .finish(),
        }
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ClaudeCredentialBundlePayload<'a> {
    version: u8,
    provider: &'static str,
    access_token: &'a str,
    refresh_token: &'a str,
    expires_at: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedClaudeCredentialBundlePayload {
    version: u8,
    provider: String,
    access_token: String,
    refresh_token: String,
    expires_at: u64,
}

fn invalid_bundle(message: &str) -> SecretStoreError {
    SecretStoreError::InvalidSecretPayload {
        message: message.to_owned(),
    }
}

fn secret_payload_error(error: impl std::fmt::Display) -> SecretStoreError {
    SecretStoreError::InvalidSecretPayload {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use codex_router_core::provider::Provider;
    use codex_router_core::redaction::SecretString;

    use super::CredentialBundle;
    use crate::account_tokens::AccountCredentialBundle;

    #[test]
    fn claude_bundle_round_trips_provider_tokens_and_expiry_without_debug_secrets() {
        let bundle = CredentialBundle::new_claude(
            SecretString::new("claude-access-canary"),
            SecretString::new("claude-refresh-canary"),
            1_800_000_000,
        )
        .expect("valid Claude token pair should form a bundle");

        let encoded = bundle
            .to_secret_string()
            .expect("Claude bundle should serialize into the encrypted store payload");
        let decoded = CredentialBundle::from_secret_string(Provider::Claude, encoded)
            .expect("Claude payload should decode as the Claude provider variant");

        assert_eq!(decoded.provider(), Provider::Claude);
        assert_eq!(
            decoded.access_token().expose_secret(),
            "claude-access-canary"
        );
        assert_eq!(
            decoded.refresh_token().map(SecretString::expose_secret),
            Some("claude-refresh-canary")
        );
        assert_eq!(decoded.expires_unix_seconds(), Some(1_800_000_000));
        let debug = format!("{decoded:?}");
        assert!(!debug.contains("claude-access-canary"));
        assert!(!debug.contains("claude-refresh-canary"));
    }

    #[test]
    fn provider_scoped_decoder_rejects_a_claude_payload_under_an_openai_key() {
        let bundle = CredentialBundle::new_claude(
            SecretString::new("claude-access-canary"),
            SecretString::new("claude-refresh-canary"),
            1_800_000_000,
        )
        .expect("valid Claude token pair should form a bundle");
        let encoded = bundle
            .to_secret_string()
            .expect("Claude bundle should serialize");

        assert!(
            CredentialBundle::from_secret_string(Provider::Openai, encoded).is_err(),
            "provider mismatch must fail closed"
        );
    }

    #[test]
    fn openai_bundle_payload_remains_identical_to_the_pr2_payload() {
        let openai = AccountCredentialBundle::imported_codex_auth(
            "openai-access-canary",
            Some("openai-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(1_800_000_000);
        let old_payload = openai
            .to_secret_string()
            .expect("OpenAI legacy bundle should serialize");
        let generic = CredentialBundle::from(openai);
        let new_payload = generic
            .to_secret_string()
            .expect("generic OpenAI variant should serialize");

        assert_eq!(
            new_payload.expose_secret(),
            old_payload.expose_secret(),
            "introducing Claude must not rewrite PR2's stored OpenAI payload"
        );
    }
}
