//! Static observation of an already-returned platform key-read result.

use super::{KeychainAccessError, platform_key_read_status_code};

pub(super) fn map_platform_key_read_result(
    result: Result<Vec<u8>, keyring_core::Error>,
) -> Result<Option<Vec<u8>>, KeychainAccessError> {
    match result {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(error) => {
            let failure = PlatformKeyReadFailure::from_returned_error(&error);
            tracing::error!(
                error.kind = failure.label(),
                "pooled credential Keychain read failed"
            );
            Err(KeychainAccessError::Unavailable)
        }
    }
}

enum PlatformKeyReadFailure {
    InteractionNotAllowed,
    InteractionRequired,
    AuthFailed,
    UserCanceled,
    KeychainNotAvailable,
    NoSuchKeychain,
    InvalidKeychain,
    NoDefaultKeychain,
    KeyDataNotAvailable,
    MissingEntitlement,
    PlatformFailureUnclassified,
    StorageAccessUnclassified,
    InvalidKeyEncoding,
    InvalidKeyData,
    InvalidStoreFormat,
    InvalidLocator,
    KeyringFailureUnclassified,
}

impl PlatformKeyReadFailure {
    fn from_returned_error(error: &keyring_core::Error) -> Self {
        // SecBase.h defines these statuses. Unknown numbers and platform
        // payloads remain private; a label never claims a more specific cause.
        match platform_key_read_status_code(error) {
            Some(-25308) => return Self::InteractionNotAllowed,
            Some(-25315) => return Self::InteractionRequired,
            Some(-25293) => return Self::AuthFailed,
            Some(-128) => return Self::UserCanceled,
            Some(-25291) => return Self::KeychainNotAvailable,
            Some(-25294) => return Self::NoSuchKeychain,
            Some(-25295) => return Self::InvalidKeychain,
            Some(-25307) => return Self::NoDefaultKeychain,
            Some(-25316) => return Self::KeyDataNotAvailable,
            Some(-34018) => return Self::MissingEntitlement,
            _ => {}
        }
        match error {
            keyring_core::Error::PlatformFailure(_) => Self::PlatformFailureUnclassified,
            keyring_core::Error::NoStorageAccess(_) => Self::StorageAccessUnclassified,
            keyring_core::Error::BadEncoding(_) => Self::InvalidKeyEncoding,
            keyring_core::Error::BadDataFormat(_, _) => Self::InvalidKeyData,
            keyring_core::Error::BadStoreFormat(_) => Self::InvalidStoreFormat,
            keyring_core::Error::Invalid(_, _) => Self::InvalidLocator,
            _ => Self::KeyringFailureUnclassified,
        }
    }

    const fn label(&self) -> &'static str {
        match self {
            Self::InteractionNotAllowed => "interaction_not_allowed",
            Self::InteractionRequired => "interaction_required",
            Self::AuthFailed => "auth_failed",
            Self::UserCanceled => "user_canceled",
            Self::KeychainNotAvailable => "keychain_not_available",
            Self::NoSuchKeychain => "no_such_keychain",
            Self::InvalidKeychain => "invalid_keychain",
            Self::NoDefaultKeychain => "no_default_keychain",
            Self::KeyDataNotAvailable => "key_data_not_available",
            Self::MissingEntitlement => "missing_entitlement",
            Self::PlatformFailureUnclassified => "platform_failure_unclassified",
            Self::StorageAccessUnclassified => "storage_access_unclassified",
            Self::InvalidKeyEncoding => "invalid_key_encoding",
            Self::InvalidKeyData => "invalid_key_data",
            Self::InvalidStoreFormat => "invalid_store_format",
            Self::InvalidLocator => "invalid_locator",
            Self::KeyringFailureUnclassified => "keyring_failure_unclassified",
        }
    }
}

#[cfg(test)]
#[path = "platform_key_read_diagnostic_tests.rs"]
mod tests;
