use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::keychain_data_key::KeychainAccess;
use codex_router_secret_store::keychain_data_key::KeychainAccessError;
use codex_router_secret_store::keychain_data_key::ROUTER_KEYCHAIN_SERVICE;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use tempfile::TempDir;

use super::*;
use crate::quota_reset::FixedOriginInteractiveResetSessionFactory;
use crate::quota_reset::InteractiveResetSessionFactory;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus;

#[derive(Default)]
struct CountingKeychainAccess {
    items: Mutex<HashMap<(String, String), Vec<u8>>>,
    read_count: AtomicUsize,
    add_count: AtomicUsize,
}

impl KeychainAccess for CountingKeychainAccess {
    fn read_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        self.read_count.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .items
            .lock()
            .expect("Keychain items lock")
            .get(&(service.to_owned(), account.to_owned()))
            .cloned())
    }

    fn add_secret(
        &self,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        let mut items = self.items.lock().expect("Keychain items lock");
        if items.contains_key(&(service.to_owned(), account.to_owned())) {
            return Err(KeychainAccessError::Unavailable);
        }
        items.insert((service.to_owned(), account.to_owned()), secret.to_vec());
        self.add_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn quota_status_reload_and_reset_session_reuse_the_process_credential_store() {
    let test_root = TempDir::new().expect("quota status root");
    let router_root = test_root.path().join("router");
    std::fs::create_dir_all(&router_root).expect("router root");
    let state = AsyncSqliteStateStore::open(&router_root.join("state.sqlite"))
        .await
        .expect("state store");
    state.close().await.expect("state close");

    let secret_root = router_root.join("secrets");
    std::fs::create_dir_all(&secret_root).expect("secret root");
    std::fs::write(secret_root.join("format-v2.marker"), b"2\n").expect("format marker");
    let keychain = CountingKeychainAccess::default();
    let credential_store =
        EncryptedCredentialStore::open_for_process_with_keychain(&secret_root, &keychain)
            .expect("process credential store");
    assert_eq!(
        credential_store.status(),
        EncryptedCredentialStoreStatus::Ready
    );
    let reads_after_open = keychain.read_count.load(Ordering::SeqCst);
    let adds_after_open = keychain.add_count.load(Ordering::SeqCst);
    assert!(reads_after_open > 0, "startup should read the Keychain key");

    let credential_resources = QuotaCredentialResources::from_opened_store(credential_store);
    let report = load_quota_status_report_with_availability_async(
        &router_root,
        false,
        1_000,
        false,
        credential_resources.availability(),
    )
    .await
    .expect("initial quota report");
    assert_eq!(
        report.credential_store_availability,
        CredentialStoreAvailability::Ready
    );

    let reload_view_model = quota_status_view_model_loader(
        router_root.clone(),
        false,
        false,
        100,
        credential_resources.availability(),
    );
    assert!(reload_view_model().await.is_some());

    let reset_session = FixedOriginInteractiveResetSessionFactory
        .create(
            &router_root,
            credential_resources
                .credential_store()
                .expect("opened process store"),
        )
        .expect("reset session composition");
    drop(reset_session);

    assert_eq!(keychain.read_count.load(Ordering::SeqCst), reads_after_open);
    assert_eq!(keychain.add_count.load(Ordering::SeqCst), adds_after_open);
}
