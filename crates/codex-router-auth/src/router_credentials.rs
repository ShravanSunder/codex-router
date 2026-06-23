//! Router-owned credential import model for Codex auth files.

use serde::Deserialize;
use thiserror::Error;

/// Imported OAuth material ready to copy into router-owned storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouterImportedCredentials {
    access_token: String,
    refresh_token: Option<String>,
    expires_at_unix_seconds: Option<u64>,
}

impl RouterImportedCredentials {
    /// Returns access token material.
    #[must_use]
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// Returns refresh token material when present.
    #[must_use]
    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    /// Returns token expiry when present.
    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> Option<u64> {
        self.expires_at_unix_seconds
    }
}

/// Router credential import failure.
#[derive(Debug, Error)]
pub enum RouterCredentialImportError {
    /// Stored auth file was not valid JSON.
    #[error("failed to parse auth json: {message}")]
    ParseAuth {
        /// Redacted parser message.
        message: String,
    },
    /// API-key auth is not router-importable OAuth material.
    #[error("router account import requires Codex OAuth auth.json tokens, not API-key auth")]
    ApiKeyAuth,
    /// OAuth access token is absent.
    #[error("access token not found in auth json")]
    MissingAccessToken,
    /// Expiry field was present but unusable.
    #[error("token expiry must be a non-negative Unix timestamp")]
    InvalidExpiry,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct StoredAuthForImport {
    auth_mode: Option<String>,
    tokens: Option<StoredTokensForImport>,
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct StoredTokensForImport {
    access_token: Option<String>,
    refresh_token: Option<String>,
    #[serde(alias = "expires_at", alias = "expiry")]
    expires_at_unix_seconds: Option<i64>,
}

/// Parses auth.json text into router-owned OAuth credential material.
pub fn router_credentials_from_auth_text(
    content: &str,
) -> Result<RouterImportedCredentials, RouterCredentialImportError> {
    let stored_auth: StoredAuthForImport =
        serde_json::from_str(content).map_err(|error| RouterCredentialImportError::ParseAuth {
            message: error.to_string(),
        })?;
    router_credentials_from_stored_auth(&stored_auth)
}

fn router_credentials_from_stored_auth(
    stored_auth: &StoredAuthForImport,
) -> Result<RouterImportedCredentials, RouterCredentialImportError> {
    let has_api_key = stored_auth
        .openai_api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty());
    let auth_mode = stored_auth.auth_mode.as_deref().map(normalize_auth_mode);
    if auth_mode.as_deref() == Some("apikey") || has_api_key {
        return Err(RouterCredentialImportError::ApiKeyAuth);
    }

    let tokens = stored_auth
        .tokens
        .as_ref()
        .ok_or(RouterCredentialImportError::MissingAccessToken)?;
    let access_token = trimmed_nonempty(tokens.access_token.as_deref())
        .ok_or(RouterCredentialImportError::MissingAccessToken)?
        .to_owned();
    let refresh_token = trimmed_nonempty(tokens.refresh_token.as_deref()).map(str::to_owned);
    let expires_at_unix_seconds = tokens
        .expires_at_unix_seconds
        .map(u64::try_from)
        .transpose()
        .map_err(|_| RouterCredentialImportError::InvalidExpiry)?;

    Ok(RouterImportedCredentials {
        access_token,
        refresh_token,
        expires_at_unix_seconds,
    })
}

fn trimmed_nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn normalize_auth_mode(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !matches!(character, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::RouterCredentialImportError;
    use super::router_credentials_from_auth_text;

    #[test]
    fn import_credentials_accept_access_token_only_auth_json() {
        let credentials = match router_credentials_from_auth_text(
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":" access-token "}}"#,
        ) {
            Ok(credentials) => credentials,
            Err(error) => panic!("access-token auth should parse: {error}"),
        };

        assert_eq!(credentials.access_token(), "access-token");
        assert_eq!(credentials.refresh_token(), None);
        assert_eq!(credentials.expires_at_unix_seconds(), None);
    }

    #[test]
    fn import_credentials_accept_refresh_token_and_expiry() {
        let credentials = match router_credentials_from_auth_text(
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"access","refresh_token":" refresh ","expires_at":2000}}"#,
        ) {
            Ok(credentials) => credentials,
            Err(error) => panic!("refresh-token auth should parse: {error}"),
        };

        assert_eq!(credentials.access_token(), "access");
        assert_eq!(credentials.refresh_token(), Some("refresh"));
        assert_eq!(credentials.expires_at_unix_seconds(), Some(2_000));
    }

    #[test]
    fn import_credentials_reject_api_key_auth_without_printing_key() {
        let error = match router_credentials_from_auth_text(
            r#"{"auth_mode":"api_key","OPENAI_API_KEY":"sk-local-secret-canary"}"#,
        ) {
            Ok(credentials) => panic!("api-key auth should reject: {credentials:?}"),
            Err(error) => error,
        };

        assert!(matches!(error, RouterCredentialImportError::ApiKeyAuth));
        assert!(!format!("{error:?}").contains("sk-local-secret-canary"));
    }

    #[test]
    fn import_credentials_reject_malformed_json_without_token_values() {
        let error = match router_credentials_from_auth_text(
            r#"{"tokens":{"access_token":"access-token-canary"}"#,
        ) {
            Ok(credentials) => panic!("malformed auth should reject: {credentials:?}"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            RouterCredentialImportError::ParseAuth { .. }
        ));
        assert!(!format!("{error:?}").contains("access-token-canary"));
    }
}
