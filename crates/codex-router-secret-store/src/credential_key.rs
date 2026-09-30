//! Provider-scoped identity for one pooled account credential generation.

use std::num::NonZeroU64;

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;

use crate::model::SecretKey;
use crate::model::SecretStoreError;

/// Provider, account, and generation that own one encrypted credential bundle.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AccountCredentialKey {
    provider: Provider,
    account_id: AccountId,
    generation: NonZeroU64,
}

impl AccountCredentialKey {
    /// Creates a provider-scoped key for one nonzero credential generation.
    pub fn new(
        provider: Provider,
        account_id: AccountId,
        generation: u64,
    ) -> Result<Self, SecretStoreError> {
        let generation =
            NonZeroU64::new(generation).ok_or(SecretStoreError::InvalidCredentialKey {
                key: format!(
                    "{}_credential_bundle.{}.0",
                    provider.as_str(),
                    account_id.as_str()
                ),
            })?;
        Ok(Self {
            provider,
            account_id,
            generation,
        })
    }

    /// Parses a pooled credential key. Non-credential keys return `Ok(None)`.
    pub fn parse(secret_key: &SecretKey) -> Result<Option<Self>, SecretStoreError> {
        let value = secret_key.as_str();
        if !value.contains("_credential_bundle") {
            return Ok(None);
        }
        let (provider_name, remainder) =
            value.split_once("_credential_bundle.").ok_or_else(|| {
                SecretStoreError::InvalidCredentialKey {
                    key: value.to_owned(),
                }
            })?;
        let provider = Provider::parse(provider_name).ok_or_else(|| {
            SecretStoreError::UnknownCredentialProvider {
                provider: provider_name.to_owned(),
            }
        })?;
        let (account_name, generation_text) =
            remainder
                .rsplit_once('.')
                .ok_or_else(|| SecretStoreError::InvalidCredentialKey {
                    key: value.to_owned(),
                })?;
        let account_id =
            AccountId::new(account_name).map_err(|_| SecretStoreError::InvalidCredentialKey {
                key: value.to_owned(),
            })?;
        let generation =
            generation_text
                .parse::<u64>()
                .map_err(|_| SecretStoreError::InvalidCredentialKey {
                    key: value.to_owned(),
                })?;
        Self::new(provider, account_id, generation).map(Some)
    }

    /// Returns the owning provider.
    #[must_use]
    pub const fn provider(&self) -> Provider {
        self.provider
    }

    /// Returns the owning account.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the positive generation number.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// Returns the canonical provider-scoped file key.
    pub fn secret_key(&self) -> Result<SecretKey, SecretStoreError> {
        SecretKey::new(format!(
            "{}_credential_bundle.{}.{}",
            self.provider.as_str(),
            self.account_id.as_str(),
            self.generation
        ))
    }
}

/// Returns whether a key resembles a pooled credential key, including malformed keys.
pub(crate) fn has_credential_bundle_marker(secret_key: &SecretKey) -> bool {
    secret_key.as_str().contains("_credential_bundle")
}
