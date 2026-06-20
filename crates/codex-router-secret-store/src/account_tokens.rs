//! Secret-key conventions for upstream OpenAI account token material.

use codex_router_core::ids::AccountId;

use crate::model::SecretKey;
use crate::model::SecretStoreError;

/// Builds the secret key for an account's upstream OpenAI access token.
pub fn upstream_access_token_key(account_id: &AccountId) -> Result<SecretKey, SecretStoreError> {
    SecretKey::new(format!("openai_access_token.{}", account_id.as_str()))
}
