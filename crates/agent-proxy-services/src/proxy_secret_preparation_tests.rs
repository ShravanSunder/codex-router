use crate::proxy_role_test_fixtures::*;
use crate::proxy_secret_preparation::prepare_with_keychain;
use crate::*;
use codex_router_keeper_protocol::{ChildComponent, ChildDegradation, PrepareMode};
use codex_router_secret_store::keychain_data_key::{KeychainAccess, KeychainAccessError};
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
#[derive(Default)]
struct RecordingKeychain {
    keys: Mutex<HashMap<String, Vec<u8>>>,
    adds: AtomicUsize,
    interactive_reads: AtomicUsize,
    noninteractive_reads: AtomicUsize,
}
impl KeychainAccess for RecordingKeychain {
    fn read_secret(
        &self,
        _service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        self.interactive_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .keys
            .lock()
            .expect("fixture key lock")
            .get(account)
            .cloned())
    }
    fn read_secret_without_user_interaction(
        &self,
        _service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        self.noninteractive_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .keys
            .lock()
            .expect("fixture key lock")
            .get(account)
            .cloned())
    }
    fn add_secret(
        &self,
        _service: &str,
        account: &str,
        bytes: &[u8],
    ) -> Result<(), KeychainAccessError> {
        self.adds.fetch_add(1, Ordering::SeqCst);
        assert!(
            self.keys
                .lock()
                .expect("fixture key lock")
                .insert(account.to_owned(), bytes.to_vec())
                .is_none(),
            "never spend creation twice"
        );
        Ok(())
    }
}
#[test]
fn fresh_key_creation_once_then_replacement_uses_only_noninteractive_read() {
    let root = tempfile::tempdir().expect("isolated key root");
    let secrets = root.path().join("secrets");
    let keychain = RecordingKeychain::default();
    let first = prepare_with_keychain(&secrets, &PrepareMode::Fresh, &keychain)
        .expect("actual Fresh creators");
    let before = snapshot(root.path());
    let second = prepare_with_keychain(&secrets, &PrepareMode::Fresh, &keychain)
        .expect("second existing handle");
    assert_eq!(keychain.adds.load(Ordering::SeqCst), 1);
    assert_eq!(keychain.keys.lock().expect("keys").len(), 1);
    assert_eq!(snapshot(root.path()), before);
    keychain.interactive_reads.store(0, Ordering::SeqCst);
    let replacement = prepare_with_keychain(
        &secrets,
        &PrepareMode::Replacement {
            active_degraded: Vec::new(),
        },
        &keychain,
    )
    .expect("existing-only owner");
    assert_eq!(keychain.interactive_reads.load(Ordering::SeqCst), 0);
    assert_eq!(keychain.noninteractive_reads.load(Ordering::SeqCst), 1);
    assert_eq!(keychain.adds.load(Ordering::SeqCst), 1);
    assert_eq!(snapshot(root.path()), before);
    drop((first, second, replacement));
}
struct UnavailableKeychain;
impl KeychainAccess for UnavailableKeychain {
    fn read_secret(
        &self,
        _service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        panic!("Replacement must never call interactive read")
    }
    fn read_secret_without_user_interaction(
        &self,
        _service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        Err(KeychainAccessError::Unavailable)
    }
    fn add_secret(
        &self,
        _service: &str,
        _account: &str,
        _bytes: &[u8],
    ) -> Result<(), KeychainAccessError> {
        panic!("Replacement must never create a Keychain item")
    }
}
#[test]
fn only_reported_pooled_credentials_unavailability_permits_the_same_degradation() {
    let root = tempfile::tempdir().expect("isolated degraded root");
    let secret_root = root.path().join("secrets");
    let keychain = RecordingKeychain::default();
    drop(
        prepare_with_keychain(&secret_root, &PrepareMode::Fresh, &keychain)
            .expect("real local prerequisites"),
    );
    let before = snapshot(root.path());
    let refusal = prepare_with_keychain(
        &secret_root,
        &PrepareMode::Replacement {
            active_degraded: Vec::new(),
        },
        &UnavailableKeychain,
    );
    assert!(matches!(
        refusal,
        Err(ProxyPreparationError::CredentialsUnavailable)
    ));
    assert_eq!(snapshot(root.path()), before);
    let accepted = prepare_with_keychain(
        &secret_root,
        &PrepareMode::Replacement {
            active_degraded: vec![(
                ChildComponent::PooledCredentials,
                ChildDegradation::CredentialStoreUnavailable,
            )],
        },
        &UnavailableKeychain,
    )
    .expect("same active degradation only");
    assert_eq!(
        accepted.degraded,
        vec![(
            ChildComponent::PooledCredentials,
            ChildDegradation::CredentialStoreUnavailable
        )]
    );
    assert_eq!(snapshot(root.path()), before);
}
