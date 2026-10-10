//! Secret-store model types.

use std::path::PathBuf;

use thiserror::Error;

/// Secret key used as a safe file stem under the router-owned root.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SecretKey(String);

impl SecretKey {
    /// Builds a secret key from a conservative file-safe string.
    pub fn new(value: impl Into<String>) -> Result<Self, SecretStoreError> {
        let value = value.into();
        if value.is_empty() || !value.chars().all(is_allowed_key_char) {
            return Err(SecretStoreError::InvalidSecretKey { value });
        }

        Ok(Self(value))
    }

    /// Returns the key's file-safe representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Secret-store operation error.
#[derive(Debug, Error)]
pub enum SecretStoreError {
    /// Plaintext pooled credentials require an explicit isolated debug declaration.
    #[error("debug plaintext credential storage is unavailable or undeclared")]
    DebugPlaintextUnavailable,
    /// The chosen root overlaps normal Router storage or another storage policy.
    #[error("debug plaintext credential root is not isolated or has conflicting storage metadata")]
    DebugPlaintextRootRejected,
    /// The root-local debug policy declaration is not the supported exact value.
    #[error("debug plaintext credential declaration is invalid")]
    InvalidDebugPlaintextDeclaration,
    /// Secret key contained unsupported characters.
    #[error("invalid secret key: {value}")]
    InvalidSecretKey {
        /// Rejected key.
        value: String,
    },

    /// Pooled credential keys must be written through the encrypted store.
    #[error("pooled credential key requires EncryptedCredentialStore: {key}")]
    PooledCredentialRequiresEncryption {
        /// Rejected file-store key.
        key: String,
    },

    /// Pooled credential key did not match its provider/account/generation shape.
    #[error("invalid pooled credential key: {key}")]
    InvalidCredentialKey {
        /// Rejected provider-scoped key.
        key: String,
    },

    /// A pooled credential file name resolved to a non-regular file.
    #[error("unexpected pooled credential filesystem entry: {path}")]
    UnexpectedCredentialEntry {
        /// Entry that stopped migration.
        path: PathBuf,
    },

    /// Pooled credential key named an unknown provider.
    #[error("unknown pooled credential provider: {provider}")]
    UnknownCredentialProvider {
        /// Rejected provider name.
        provider: String,
    },

    /// Operating-system randomness could not provide a nonce or data key.
    #[error("cryptographic randomness unavailable")]
    RandomnessUnavailable(#[source] getrandom::Error),

    /// Keychain did not provide the pooled-credential key.
    #[error("pooled credential key unavailable (Keychain locked or access denied)")]
    KeyUnavailable,

    /// No Keychain key exists although encrypted pooled credential files do.
    #[error("pooled credential key is missing while encrypted credentials exist")]
    KeyMissing,

    /// Stored Keychain data key was not exactly 32 bytes.
    #[error("stored pooled credential key has an invalid length")]
    InvalidDataKey,

    /// Production Keychain access was asked to use a service other than Router's own service.
    #[error("Keychain service is not owned by codex-router")]
    KeychainServiceRejected,

    /// A test build attempted to use the production Keychain adapter.
    #[error(
        "production Keychain access is forbidden in tests; inject a KeychainAccess test double"
    )]
    TestKeychainAccessForbidden,

    /// A deterministic test credential key was requested for the production root.
    #[error("test credential key is forbidden for the production router secret root")]
    TestCredentialKeyOnProductionRoot,

    /// The production home could not be resolved to guard a deterministic test key.
    #[error("test credential key cannot verify the production router secret root")]
    TestCredentialKeyRootUnverifiable,

    /// Store metadata did not contain a valid identifier or format marker.
    #[error("invalid pooled credential store metadata: {path}")]
    InvalidCredentialStoreMarker {
        /// Invalid metadata file.
        path: PathBuf,
    },

    /// A cross-process credential-store lock could not be acquired.
    #[error("pooled credential store lock failed at {path}: {source}")]
    CredentialStoreLock {
        /// Lock file path.
        path: PathBuf,
        /// Underlying file-lock or I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The pooled store is unavailable for a durable store-level reason.
    #[error("pooled credential store unavailable: {0}")]
    StoreUnavailable(#[from] StoreUnavailable),

    /// Encrypted credential envelope was malformed or used an unsupported format.
    #[error("invalid encrypted credential envelope")]
    InvalidCredentialEnvelope,

    /// AEAD initialization failed for the fixed AES-256 key size.
    #[error("encrypted credential cipher initialization failed")]
    CipherInitializationFailed,

    /// Ciphertext authentication failed for the supplied nonce, key name, or data key.
    #[error("encrypted credential authentication failed")]
    CredentialAuthenticationFailed,

    /// Credential plaintext could not be encoded as UTF-8.
    #[error("decrypted credential payload was not valid UTF-8")]
    InvalidCredentialEncoding,

    /// Router root must never live inside Codex home.
    #[error("secret store root must not use .codex path: {path}")]
    CodexHomePath {
        /// Rejected path.
        path: PathBuf,
    },

    /// Symlinks are rejected for secret-store paths.
    #[error("secret store refuses symlink path: {path}")]
    SymlinkPath {
        /// Rejected path.
        path: PathBuf,
    },

    /// Filesystem operation failed.
    #[error("secret store filesystem error at {path}: {source}")]
    Filesystem {
        /// Path being accessed.
        path: PathBuf,
        /// Source IO error.
        #[source]
        source: std::io::Error,
    },

    /// Stored secret payload was malformed.
    #[error("invalid secret payload: {message}")]
    InvalidSecretPayload {
        /// Redacted parse or validation message.
        message: String,
    },
}

/// Safe reason that prevents a pooled-credential migration from completing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum CredentialMigrationFailure {
    /// The store marker or credential file names were malformed.
    #[error("credential store metadata or file names are invalid")]
    InvalidStoreData,
    /// Migration metadata could not be enumerated or read.
    #[error("credential migration metadata could not be read")]
    MetadataReadFailed,
    /// The migration marker is absent while pooled credential files remain.
    #[error("credential migration has not completed")]
    MigrationNotComplete,
    /// A legacy credential or encrypted envelope could not be read.
    #[error("credential file read failed")]
    CredentialReadFailed,
    /// An encrypted envelope could not be published.
    #[error("encrypted credential write failed")]
    EnvelopeWriteFailed,
    /// The encrypted envelope did not decrypt to the exact legacy value.
    #[error("credential read-back did not match its legacy source")]
    ReadBackMismatch,
    /// The verified legacy source could not be removed.
    #[error("verified legacy credential could not be deleted")]
    LegacyDeleteFailed,
    /// The completion marker could not be published.
    #[error("credential migration completion marker write failed")]
    MarkerWriteFailed,
    /// An unexpected directory entry or stale temp file could not be handled.
    #[error("unexpected credential filesystem entry")]
    UnexpectedEntry,
}

/// Durable condition that prevents pooled credential access for this process.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum StoreUnavailable {
    /// Legacy plaintext conversion has not completed for the named accounts.
    #[error("credential migration incomplete ({failure}) for accounts: {accounts:?}")]
    MigrationIncomplete {
        /// Accounts with credential files that have not completed conversion.
        accounts: Vec<String>,
        /// Safe reason that stopped migration.
        failure: CredentialMigrationFailure,
    },
}

fn is_allowed_key_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
}
