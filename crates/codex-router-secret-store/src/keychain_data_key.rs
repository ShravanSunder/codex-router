//! Router-owned Keychain item and zeroized pooled-credential data key.

use std::fmt;
use std::sync::Arc;

use uuid::Uuid;
use zeroize::Zeroizing;

use crate::file_backend::FileSecretStore;
use crate::model::SecretStoreError;

/// The only Keychain service accepted by Router's production adapter.
pub const ROUTER_KEYCHAIN_SERVICE: &str = "codex-router";

const POOLED_KEY_ACCOUNT_PREFIX: &str = "pooled-credential-key:";
const AES_256_KEY_LENGTH: usize = 32;

#[cfg(all(target_os = "macos", any(test, not(feature = "keychain-test-guard"))))]
#[path = "keychain_data_key/platform_key_read_diagnostic.rs"]
mod platform_key_read_diagnostic;

/// AES-256 data key shared by credential-store handles in one process.
#[derive(Clone)]
pub struct PooledCredentialDataKey(Arc<Zeroizing<[u8; AES_256_KEY_LENGTH]>>);

impl PooledCredentialDataKey {
    /// Wraps one exact AES-256 key, clearing it when the final handle drops.
    #[must_use]
    pub fn from_bytes(bytes: [u8; AES_256_KEY_LENGTH]) -> Self {
        Self(Arc::new(Zeroizing::new(bytes)))
    }

    /// Generates a data key from the operating system's cryptographic random source.
    pub fn generate() -> Result<Self, SecretStoreError> {
        let mut bytes = Zeroizing::new([0_u8; AES_256_KEY_LENGTH]);
        getrandom::fill(&mut *bytes).map_err(SecretStoreError::RandomnessUnavailable)?;
        Ok(Self::from_bytes(*bytes))
    }

    pub(crate) fn bytes(&self) -> &[u8; AES_256_KEY_LENGTH] {
        &self.0
    }
}

impl fmt::Debug for PooledCredentialDataKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("PooledCredentialDataKey")
            .field(&"[REDACTED]")
            .finish()
    }
}

/// Narrow seam for loading and creating Router's one pooled-credential Keychain item.
pub trait KeychainAccess: Send + Sync {
    /// Reads the requested item, returning `None` only when the item is absent.
    fn read_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError>;

    /// Adds an item only when the service/account pair is absent.
    fn add_secret(
        &self,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), KeychainAccessError>;
}

/// Keychain boundary failures, kept independent of platform error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeychainAccessError {
    /// The caller requested a Keychain service other than Router's own service.
    ServiceRejected,
    /// The Keychain is locked, denied, unavailable, or returned an unclassified error.
    Unavailable,
    /// A test build attempted to use the production Keychain adapter.
    TestKeychainAccessForbidden,
}

/// System Keychain implementation; non-macOS targets fail closed.
#[cfg(not(any(test, feature = "keychain-test-guard")))]
struct PlatformKeychainAccess;

#[cfg(all(target_os = "macos", not(any(test, feature = "keychain-test-guard"))))]
impl KeychainAccess for PlatformKeychainAccess {
    fn read_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        ensure_router_keychain_service(service)?;
        use apple_native_keyring_store::keychain::{Cred, MacKeychainDomain};

        let entry = Cred::build(MacKeychainDomain::User, service, account)
            .map_err(|_| KeychainAccessError::Unavailable)?;
        platform_key_read_diagnostic::map_platform_key_read_result(entry.get_secret())
    }

    fn add_secret(
        &self,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        ensure_router_keychain_service(service)?;
        use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};

        let keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User)
            .map_err(|_| KeychainAccessError::Unavailable)?;
        keychain
            .add_generic_password(service, account, secret)
            .map_err(|_| KeychainAccessError::Unavailable)
    }
}

#[cfg(all(target_os = "macos", any(test, not(feature = "keychain-test-guard"))))]
fn platform_key_read_status_code(error: &keyring_core::Error) -> Option<i32> {
    match error {
        keyring_core::Error::PlatformFailure(error)
        | keyring_core::Error::NoStorageAccess(error) => error
            .downcast_ref::<security_framework::base::Error>()
            .map(|error| error.code()),
        _ => None,
    }
}

#[cfg(all(test, target_os = "macos"))]
fn synthetic_platform_key_error(code: i32, storage_access: bool) -> keyring_core::Error {
    let error = Box::new(security_framework::base::Error::from_code(code));
    if storage_access {
        keyring_core::Error::NoStorageAccess(error)
    } else {
        keyring_core::Error::PlatformFailure(error)
    }
}

#[cfg(all(
    not(target_os = "macos"),
    not(any(test, feature = "keychain-test-guard"))
))]
impl KeychainAccess for PlatformKeychainAccess {
    fn read_secret(
        &self,
        service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        ensure_router_keychain_service(service)?;
        Err(KeychainAccessError::Unavailable)
    }

    fn add_secret(
        &self,
        service: &str,
        _account: &str,
        _secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        ensure_router_keychain_service(service)?;
        Err(KeychainAccessError::Unavailable)
    }
}

/// Runs one operation through the system Keychain adapter in production.
///
/// Test builds fail closed before constructing the adapter. Tests must inject
/// an explicit `KeychainAccess` implementation instead.
pub(crate) fn with_platform_keychain_for_production<TResult>(
    operation: impl FnOnce(&dyn KeychainAccess) -> Result<TResult, SecretStoreError>,
) -> Result<TResult, SecretStoreError> {
    #[cfg(any(test, feature = "keychain-test-guard"))]
    {
        let _ = operation;
        Err(SecretStoreError::TestKeychainAccessForbidden)
    }
    #[cfg(not(any(test, feature = "keychain-test-guard")))]
    {
        operation(&PlatformKeychainAccess)
    }
}

pub(crate) fn load_or_create_pooled_credential_data_key(
    file_store: &FileSecretStore,
    keychain: &dyn KeychainAccess,
) -> Result<PooledCredentialDataKey, SecretStoreError> {
    let store_id = match file_store.read_store_id_file()? {
        Some(value) => PooledCredentialStoreId::parse(&value, file_store)?,
        None => {
            let store_id = PooledCredentialStoreId::new();
            file_store.write_store_id_file(&store_id.to_string())?;
            store_id
        }
    };
    let keychain_account = format!("{POOLED_KEY_ACCOUNT_PREFIX}{store_id}");
    let stored_key = read_keychain_secret(keychain, &keychain_account)?;
    if let Some(secret) = stored_key {
        return data_key_from_secret(secret);
    }
    if file_store.has_any_v2_files()? {
        return Err(SecretStoreError::KeyMissing);
    }

    let generated_key = PooledCredentialDataKey::generate()?;
    match add_keychain_secret(keychain, &keychain_account, generated_key.bytes()) {
        Ok(()) => Ok(generated_key),
        Err(SecretStoreError::KeyUnavailable) => {
            // Another first opener may have published the item between the read and add.
            match read_keychain_secret(keychain, &keychain_account)? {
                Some(secret) => data_key_from_secret(secret),
                None => Err(SecretStoreError::KeyUnavailable),
            }
        }
        Err(error) => Err(error),
    }
}

fn read_keychain_secret(
    keychain: &dyn KeychainAccess,
    account: &str,
) -> Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError> {
    match keychain.read_secret(ROUTER_KEYCHAIN_SERVICE, account) {
        Ok(secret) => Ok(secret.map(Zeroizing::new)),
        Err(KeychainAccessError::ServiceRejected) => Err(SecretStoreError::KeychainServiceRejected),
        Err(KeychainAccessError::Unavailable) => Err(SecretStoreError::KeyUnavailable),
        Err(KeychainAccessError::TestKeychainAccessForbidden) => {
            Err(SecretStoreError::TestKeychainAccessForbidden)
        }
    }
}

fn add_keychain_secret(
    keychain: &dyn KeychainAccess,
    account: &str,
    secret: &[u8],
) -> Result<(), SecretStoreError> {
    match keychain.add_secret(ROUTER_KEYCHAIN_SERVICE, account, secret) {
        Ok(()) => Ok(()),
        Err(KeychainAccessError::ServiceRejected) => Err(SecretStoreError::KeychainServiceRejected),
        Err(KeychainAccessError::Unavailable) => Err(SecretStoreError::KeyUnavailable),
        Err(KeychainAccessError::TestKeychainAccessForbidden) => {
            Err(SecretStoreError::TestKeychainAccessForbidden)
        }
    }
}

fn data_key_from_secret(
    secret: Zeroizing<Vec<u8>>,
) -> Result<PooledCredentialDataKey, SecretStoreError> {
    if secret.len() != AES_256_KEY_LENGTH {
        return Err(SecretStoreError::InvalidDataKey);
    }
    let mut key_bytes = Zeroizing::new([0_u8; AES_256_KEY_LENGTH]);
    key_bytes.copy_from_slice(&secret);
    Ok(PooledCredentialDataKey::from_bytes(*key_bytes))
}

#[cfg(any(test, not(feature = "keychain-test-guard")))]
fn ensure_router_keychain_service(service: &str) -> Result<(), KeychainAccessError> {
    if service == ROUTER_KEYCHAIN_SERVICE {
        Ok(())
    } else {
        Err(KeychainAccessError::ServiceRejected)
    }
}

struct PooledCredentialStoreId(Uuid);

impl PooledCredentialStoreId {
    fn new() -> Self {
        Self(Uuid::now_v7())
    }

    fn parse(value: &str, file_store: &FileSecretStore) -> Result<Self, SecretStoreError> {
        Uuid::parse_str(value.trim()).map(Self).map_err(|_| {
            SecretStoreError::InvalidCredentialStoreMarker {
                path: file_store.root().join("store-id"),
            }
        })
    }
}

impl fmt::Display for PooledCredentialStoreId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "keychain_data_key/temporary_keychain_tests.rs"]
pub(crate) mod temporary_keychain_tests;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::KeychainAccess;
    use super::KeychainAccessError;
    use super::PooledCredentialDataKey;
    use super::ROUTER_KEYCHAIN_SERVICE;
    use super::ensure_router_keychain_service;
    use super::with_platform_keychain_for_production;

    #[derive(Default)]
    struct MemoryKeychainAccess {
        items: Mutex<HashMap<(String, String), Vec<u8>>>,
        read_count: Mutex<usize>,
    }

    impl KeychainAccess for MemoryKeychainAccess {
        fn read_secret(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
            ensure_router_keychain_service(service)?;
            *self.read_count.lock().expect("read count lock") += 1;
            Ok(self
                .items
                .lock()
                .expect("items lock")
                .get(&(service.to_owned(), account.to_owned()))
                .cloned())
        }

        fn add_secret(
            &self,
            service: &str,
            account: &str,
            secret: &[u8],
        ) -> Result<(), KeychainAccessError> {
            ensure_router_keychain_service(service)?;
            let mut items = self.items.lock().expect("items lock");
            if items.contains_key(&(service.to_owned(), account.to_owned())) {
                return Err(KeychainAccessError::Unavailable);
            }
            items.insert((service.to_owned(), account.to_owned()), secret.to_vec());
            Ok(())
        }
    }

    #[test]
    fn production_keychain_service_guard_accepts_only_router_service() {
        assert_eq!(
            ensure_router_keychain_service(ROUTER_KEYCHAIN_SERVICE),
            Ok(())
        );
        assert_eq!(
            ensure_router_keychain_service("another-application"),
            Err(KeychainAccessError::ServiceRejected)
        );
    }

    #[test]
    fn production_store_opener_fails_loudly_in_a_test_build() {
        let result = crate::encrypted_credential_store::EncryptedCredentialStore::open_production_for_process(
            std::env::temp_dir().join("codex-router-production-keychain-test-guard"),
        );

        assert!(matches!(
            result,
            Err(crate::model::SecretStoreError::TestKeychainAccessForbidden)
        ));
    }

    #[test]
    fn production_migration_entrypoint_fails_loudly_in_a_test_build() {
        let result = crate::credential_migration::migrate_pooled_credentials_at_production_startup(
            std::env::temp_dir().join("codex-router-production-migration-test-guard"),
        );

        assert!(matches!(
            result,
            Err(crate::model::SecretStoreError::TestKeychainAccessForbidden)
        ));
    }

    #[test]
    fn platform_keychain_access_is_unreachable_from_test_builds() {
        let was_called = std::cell::Cell::new(false);

        let result = with_platform_keychain_for_production(|_keychain| {
            was_called.set(true);
            Ok(())
        });

        assert!(matches!(
            result,
            Err(crate::model::SecretStoreError::TestKeychainAccessForbidden)
        ));
        assert!(
            !was_called.get(),
            "test builds must not construct the platform keychain"
        );
    }

    #[test]
    fn data_key_debug_output_is_redacted() {
        let data_key = PooledCredentialDataKey::from_bytes([0x5a; 32]);

        assert_eq!(
            format!("{data_key:?}"),
            "PooledCredentialDataKey(\"[REDACTED]\")"
        );
    }

    #[test]
    fn key_is_created_once_and_reused_for_the_same_store_root() {
        use std::sync::Arc;

        use tempfile::TempDir;

        use crate::file_backend::FileSecretStore;
        use crate::keychain_data_key::load_or_create_pooled_credential_data_key;

        let directory = TempDir::new().expect("temporary root");
        let file_store = FileSecretStore::open(directory.path()).expect("secret store");
        let keychain = Arc::new(MemoryKeychainAccess::default());

        let first_key = load_or_create_pooled_credential_data_key(&file_store, keychain.as_ref())
            .expect("first opener creates key");
        let second_key = load_or_create_pooled_credential_data_key(&file_store, keychain.as_ref())
            .expect("next opener reads key");

        assert_eq!(first_key.bytes(), second_key.bytes());
        assert_eq!(*keychain.read_count.lock().expect("read count lock"), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the opt-in unsandboxed temporary-Keychain test command"]
    fn keychain_access_uses_an_explicit_temporary_keychain_file() {
        use security_framework::os::macos::keychain::SecKeychain;

        use super::temporary_keychain_tests::TemporaryKeychainAccess;
        use super::temporary_keychain_tests::acquire_temporary_keychain_test_lock;

        assert!(matches!(
            std::env::var("CODEX_ROUTER_KEYCHAIN_TESTS").as_deref(),
            Ok("1")
        ));
        let _keychain_test_lock = acquire_temporary_keychain_test_lock();
        let keychain = TemporaryKeychainAccess::new().expect("temporary Keychain");
        let account = "pooled-credential-key:temporary-keychain-test";

        assert!(keychain.path().is_absolute());
        assert!(!SecKeychain::user_interaction_allowed().expect("query test interaction state"));
        assert_eq!(
            keychain
                .read_secret(ROUTER_KEYCHAIN_SERVICE, account)
                .expect("read temporary item"),
            None
        );
        keychain
            .add_secret(ROUTER_KEYCHAIN_SERVICE, account, &[0x37; 32])
            .expect("write temporary item");
        assert_eq!(
            keychain
                .read_secret(ROUTER_KEYCHAIN_SERVICE, account)
                .expect("read temporary item"),
            Some(vec![0x37; 32])
        );
    }
}
