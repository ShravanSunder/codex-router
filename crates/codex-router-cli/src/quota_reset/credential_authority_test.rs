use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::sqlite::AsyncSqliteStateStore;

use super::*;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_blocking_credential_read_holds_capacity_until_supervised_drain() {
    let read_permits = Arc::new(Semaphore::new(1));
    let active_reads = Arc::new(AtomicUsize::new(0));
    let maximum_active_reads = Arc::new(AtomicUsize::new(0));
    let (first_started_sender, first_started_receiver) = tokio::sync::oneshot::channel();
    let (release_first_sender, release_first_receiver) = std::sync::mpsc::channel();
    let (first_dropped_sender, mut first_dropped_receiver) = tokio::sync::oneshot::channel();
    let first_permit = Arc::clone(&read_permits)
        .acquire_owned()
        .await
        .unwrap_or_else(|error| panic!("first read permit should acquire: {error}"));
    let first_active_reads = Arc::clone(&active_reads);
    let first_maximum_active_reads = Arc::clone(&maximum_active_reads);
    let mut first_read = start_bounded_blocking_read(first_permit, move || {
        let _active_read = HeldRead::start(
            first_active_reads,
            first_maximum_active_reads,
            first_dropped_sender,
        );
        first_started_sender
            .send(())
            .unwrap_or_else(|()| panic!("test should await the first read start"));
        release_first_receiver
            .recv()
            .unwrap_or_else(|error| panic!("first held read should be released: {error}"));
        1_u8
    });
    first_started_receiver
        .await
        .unwrap_or_else(|error| panic!("first read should announce start: {error}"));
    let (cancel_sender, cancel_receiver) = tokio::sync::oneshot::channel();
    cancel_sender
        .send(())
        .unwrap_or_else(|()| panic!("cancellation receiver should remain live"));
    tokio::select! {
        biased;
        cancellation = cancel_receiver => {
            cancellation.unwrap_or_else(|error| panic!("cancellation should arrive: {error}"));
        }
        completed = first_read.drain() => {
            panic!("held read completed before cancellation: {completed:?}");
        }
    }
    let (cleanup_sender, mut cleanup_receiver) = tokio::sync::oneshot::channel();
    let cleanup_task = tokio::spawn(async move {
        let _result = first_read
            .drain()
            .await
            .unwrap_or_else(|error| panic!("cancelled read should drain: {error}"));
        cleanup_sender
            .send(())
            .unwrap_or_else(|()| panic!("test should await cleanup completion"));
    });
    assert_eq!(read_permits.available_permits(), 0);
    assert_eq!(active_reads.load(Ordering::SeqCst), 1);
    assert!(matches!(
        first_dropped_receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        cleanup_receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(Arc::clone(&read_permits).try_acquire_owned().is_err());
    assert_eq!(maximum_active_reads.load(Ordering::SeqCst), 1);
    release_first_sender
        .send(())
        .unwrap_or_else(|error| panic!("first held read should accept release: {error}"));
    first_dropped_receiver
        .await
        .unwrap_or_else(|error| panic!("first held read should drop: {error}"));
    cleanup_receiver
        .await
        .unwrap_or_else(|error| panic!("cleanup should follow first read drop: {error}"));
    cleanup_task
        .await
        .unwrap_or_else(|error| panic!("cleanup task should finish: {error}"));
    assert_eq!(maximum_active_reads.load(Ordering::SeqCst), 1);
    assert_eq!(active_reads.load(Ordering::SeqCst), 0);
    assert_eq!(read_permits.available_permits(), 1);
}

struct HeldRead {
    active_reads: Arc<AtomicUsize>,
    dropped_sender: Option<tokio::sync::oneshot::Sender<()>>,
}
impl HeldRead {
    fn start(
        active_reads: Arc<AtomicUsize>,
        maximum_active_reads: Arc<AtomicUsize>,
        dropped_sender: tokio::sync::oneshot::Sender<()>,
    ) -> Self {
        let active = active_reads.fetch_add(1, Ordering::SeqCst) + 1;
        maximum_active_reads.fetch_max(active, Ordering::SeqCst);
        Self {
            active_reads,
            dropped_sender: Some(dropped_sender),
        }
    }
}

impl Drop for HeldRead {
    fn drop(&mut self) {
        self.active_reads.fetch_sub(1, Ordering::SeqCst);
        self.dropped_sender
            .take()
            .and_then(|sender| sender.send(()).ok());
    }
}

#[test]
fn credential_fingerprint_uses_unambiguous_length_framing() {
    let first = credential_fingerprint_for_test(&[b"ab".as_slice(), b"c".as_slice()]);
    let second = credential_fingerprint_for_test(&[b"a".as_slice(), b"bc".as_slice()]);

    assert_ne!(first, second);
}

#[tokio::test]
async fn credential_authority_fails_closed_and_binds_only_provider_effective_fields() {
    let fixture = AuthorityFixture::new("authority-contract").await;
    let expected_generation = ActiveCredentialGeneration::new(7);

    let initial = fixture
        .load(expected_generation)
        .await
        .unwrap_or_else(|error| panic!("initial authority should load: {error}"));
    assert_eq!(initial.account_id(), &fixture.account_id);
    assert_eq!(initial.active_credential_generation(), expected_generation);
    assert_eq!(initial.access_token().expose_secret(), "token-alpha");
    assert_eq!(initial.chatgpt_account_id(), "routing-alpha");
    assert_eq!(initial.expires_unix_seconds(), Some(1_000));

    fixture.write_bundle(
        "token-beta",
        "routing-alpha",
        1_000,
        "refresh-alpha",
        "source-alpha",
    );
    let token_replaced = fixture
        .load(expected_generation)
        .await
        .unwrap_or_else(|error| panic!("same-generation token replacement should load: {error}"));
    assert_ne!(initial.fingerprint(), token_replaced.fingerprint());

    fixture.write_bundle(
        "token-alpha",
        "routing-beta",
        1_000,
        "refresh-alpha",
        "source-alpha",
    );
    let routing_replaced = fixture
        .load(expected_generation)
        .await
        .unwrap_or_else(|error| panic!("same-generation routing replacement should load: {error}"));
    assert_ne!(initial.fingerprint(), routing_replaced.fingerprint());

    fixture.write_bundle(
        "token-alpha",
        "routing-alpha",
        2_000,
        "refresh-alpha",
        "source-alpha",
    );
    let expiry_replaced = fixture
        .load(expected_generation)
        .await
        .unwrap_or_else(|error| panic!("same-generation expiry replacement should load: {error}"));
    assert_ne!(initial.fingerprint(), expiry_replaced.fingerprint());

    fixture.write_bundle(
        "token-alpha",
        "routing-alpha",
        1_000,
        "refresh-beta",
        "source-beta",
    );
    let non_provider_fields_replaced = fixture
        .load(expected_generation)
        .await
        .unwrap_or_else(|error| panic!("refresh and source replacement should load: {error}"));
    assert_eq!(
        initial.fingerprint(),
        non_provider_fields_replaced.fingerprint()
    );

    fixture.set_account(AccountStatus::Disabled, Some(7)).await;
    assert!(matches!(
        fixture.load(expected_generation).await,
        Err(CredentialAuthorityError::AccountUnavailable)
    ));
    fixture.set_account(AccountStatus::Enabled, Some(8)).await;
    assert!(matches!(
        fixture.load(expected_generation).await,
        Err(CredentialAuthorityError::GenerationChanged)
    ));
    fixture.set_account(AccountStatus::Enabled, Some(7)).await;
    fixture.write_bundle("token-alpha", "routing-alpha", 100, "refresh", "source");
    assert!(matches!(
        fixture.load(expected_generation).await,
        Err(CredentialAuthorityError::Expired)
    ));
    fixture.write_bundle_without_routing("token-alpha", 1_000);
    assert!(matches!(
        fixture.load(expected_generation).await,
        Err(CredentialAuthorityError::MissingRoutingId)
    ));

    let missing_account = AccountId::new("acct_missing")
        .unwrap_or_else(|error| panic!("fixture account id should parse: {error}"));
    assert!(matches!(
        load_reset_credential_authority(
            &fixture.database_path,
            &fixture.secret_root,
            &missing_account,
            expected_generation,
            200,
        )
        .await,
        Err(CredentialAuthorityError::AccountUnavailable)
    ));

    let debug = format!("{initial:?}");
    assert!(!debug.contains("acct_authority"));
    assert!(!debug.contains("token-alpha"));
    assert!(!debug.contains("routing-alpha"));
    assert!(!debug.contains("1000"));
    assert_eq!(
        format!("{:?}", initial.fingerprint()),
        "CredentialFingerprint([REDACTED])"
    );
}

#[tokio::test]
async fn authority_read_preserves_state_bytes_and_complete_secret_manifest() {
    let fixture = AuthorityFixture::new("manifest").await;
    let state_before = fs::read(&fixture.database_path)
        .unwrap_or_else(|error| panic!("fixture state bytes should load: {error}"));
    let secrets_before = recursive_manifest(&fixture.secret_root);

    let authority = fixture
        .load(ActiveCredentialGeneration::new(7))
        .await
        .unwrap_or_else(|error| panic!("read-only authority should load: {error}"));
    drop(authority);
    let state_after = fs::read(&fixture.database_path)
        .unwrap_or_else(|error| panic!("fixture state bytes should reload: {error}"));
    let secrets_after = recursive_manifest(&fixture.secret_root);

    assert_eq!(state_before, state_after);
    assert_eq!(secrets_before, secrets_after);
}

#[tokio::test]
async fn wal_reader_never_observes_uncommitted_generation_and_refreshes_after_commit() {
    let fixture = AuthorityFixture::new("wal-visibility").await;
    fixture.write_bundle_for_generation(
        8,
        "token-beta",
        "routing-beta",
        2_000,
        "refresh-beta",
        "source-beta",
    );
    let (uncommitted_ready_sender, uncommitted_ready_receiver) = tokio::sync::oneshot::channel();
    let (commit_sender, commit_receiver) = tokio::sync::oneshot::channel();
    let database_path = fixture.database_path.clone();
    let account_id = fixture.account_id.clone();
    let writer_task = tokio::spawn(async move {
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(database_path)
                    .create_if_missing(false),
            )
            .await
            .unwrap_or_else(|error| panic!("fixture writer should open: {error}"));
        let mut writer_connection = writer
            .acquire()
            .await
            .unwrap_or_else(|error| panic!("fixture writer connection should acquire: {error}"));
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *writer_connection)
            .await
            .unwrap_or_else(|error| panic!("fixture transaction should begin: {error}"));
        sqlx::query(
            "UPDATE accounts
                SET active_credential_generation = 8
              WHERE account_id = ?1",
        )
        .bind(account_id.as_str())
        .execute(&mut *writer_connection)
        .await
        .unwrap_or_else(|error| panic!("fixture uncommitted generation should write: {error}"));
        uncommitted_ready_sender
            .send(())
            .unwrap_or_else(|()| panic!("reader should await the uncommitted event"));
        commit_receiver
            .await
            .unwrap_or_else(|error| panic!("writer should receive commit event: {error}"));
        sqlx::query("COMMIT")
            .execute(&mut *writer_connection)
            .await
            .unwrap_or_else(|error| panic!("writer commit must not be delayed by reader: {error}"));
        drop(writer_connection);
        writer.close().await;
    });
    uncommitted_ready_receiver
        .await
        .unwrap_or_else(|error| panic!("writer should announce uncommitted B: {error}"));

    let read_while_uncommitted = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        fixture.load(ActiveCredentialGeneration::new(7)),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("zero-busy reader must return promptly"));
    match read_while_uncommitted {
        Ok(authority) => {
            assert_eq!(
                authority.active_credential_generation(),
                ActiveCredentialGeneration::new(7)
            );
            assert_eq!(authority.access_token().expose_secret(), "token-alpha");
        }
        Err(CredentialAuthorityError::StateReadFailed) => {}
        Err(error) => panic!("reader must return committed A or typed busy, got: {error}"),
    }

    commit_sender
        .send(())
        .unwrap_or_else(|()| panic!("writer should still await commit event"));
    writer_task
        .await
        .unwrap_or_else(|error| panic!("writer task should finish: {error}"));

    let committed = fixture
        .load(ActiveCredentialGeneration::new(8))
        .await
        .unwrap_or_else(|error| panic!("fresh read should observe committed B: {error}"));
    assert_eq!(committed.access_token().expose_secret(), "token-beta");
    assert_eq!(committed.chatgpt_account_id(), "routing-beta");
}

#[tokio::test]
async fn authority_read_does_not_create_absent_state_or_secret_roots() {
    let root = unique_test_root("absent-roots");
    let database_path = root.join("missing-state").join("state.sqlite");
    let secret_root = root.join("missing-secrets");
    let account_id = AccountId::new("acct_absent")
        .unwrap_or_else(|error| panic!("fixture account id should parse: {error}"));

    let result = load_reset_credential_authority(
        &database_path,
        &secret_root,
        &account_id,
        ActiveCredentialGeneration::new(1),
        100,
    )
    .await;

    assert!(result.is_err());
    assert!(!root.exists());
}

struct AuthorityFixture {
    root: PathBuf,
    database_path: PathBuf,
    secret_root: PathBuf,
    account_id: AccountId,
}

impl AuthorityFixture {
    async fn new(name: &str) -> Self {
        let root = unique_test_root(name);
        fs::create_dir_all(&root)
            .unwrap_or_else(|error| panic!("fixture root should create: {error}"));
        let database_path = root.join("state.sqlite");
        let secret_root = root.join("secrets");
        let account_id = AccountId::new("acct_authority")
            .unwrap_or_else(|error| panic!("fixture account id should parse: {error}"));
        let fixture = Self {
            root,
            database_path,
            secret_root,
            account_id,
        };
        fixture.set_account(AccountStatus::Enabled, Some(7)).await;
        fixture.insert_decoy_account().await;
        fixture.write_bundle(
            "token-alpha",
            "routing-alpha",
            1_000,
            "refresh-alpha",
            "source-alpha",
        );
        fixture
    }

    async fn set_account(&self, status: AccountStatus, generation: Option<u64>) {
        let state = AsyncSqliteStateStore::open(&self.database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture state should open: {error}"));
        let mut account = AccountRecord::new(self.account_id.clone(), "authority", status);
        if let Some(generation) = generation {
            account = account.with_active_credential_generation(generation);
        }
        state
            .upsert_account(&account)
            .await
            .unwrap_or_else(|error| panic!("fixture account should write: {error}"));
        state
            .close()
            .await
            .unwrap_or_else(|error| panic!("fixture state should close: {error}"));
    }

    async fn insert_decoy_account(&self) {
        let state = AsyncSqliteStateStore::open(&self.database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture state should open: {error}"));
        let decoy_id = AccountId::new("acct_decoy")
            .unwrap_or_else(|error| panic!("fixture decoy id should parse: {error}"));
        state
            .upsert_account(
                &AccountRecord::new(decoy_id, "decoy", AccountStatus::Enabled)
                    .with_active_credential_generation(99),
            )
            .await
            .unwrap_or_else(|error| panic!("fixture decoy should write: {error}"));
        state
            .close()
            .await
            .unwrap_or_else(|error| panic!("fixture state should close: {error}"));
    }

    fn write_bundle(
        &self,
        access_token: &str,
        routing_id: &str,
        expiry: u64,
        refresh_token: &str,
        source: &str,
    ) {
        self.write_bundle_for_generation(
            7,
            access_token,
            routing_id,
            expiry,
            refresh_token,
            source,
        );
    }

    fn write_bundle_for_generation(
        &self,
        generation: u64,
        access_token: &str,
        routing_id: &str,
        expiry: u64,
        refresh_token: &str,
        source: &str,
    ) {
        let bundle = AccountCredentialBundle::imported_codex_auth(
            access_token,
            Some(refresh_token.to_owned()),
        )
        .with_expires_unix_seconds(expiry)
        .with_chatgpt_account_id(routing_id);
        let serialized = bundle
            .to_secret_string()
            .unwrap_or_else(|error| panic!("fixture bundle should serialize: {error}"));
        let serialized = SecretString::new(
            serialized
                .expose_secret()
                .replace("codex_auth_json", source),
        );
        self.write_serialized_bundle(generation, &serialized);
    }

    fn write_bundle_without_routing(&self, access_token: &str, expiry: u64) {
        let bundle = AccountCredentialBundle::imported_codex_auth(access_token, None)
            .with_expires_unix_seconds(expiry);
        let serialized = bundle
            .to_secret_string()
            .unwrap_or_else(|error| panic!("fixture bundle should serialize: {error}"));
        self.write_serialized_bundle(7, &serialized);
    }

    fn write_serialized_bundle(&self, generation: u64, serialized: &SecretString) {
        let store = FileSecretStore::open(&self.secret_root)
            .unwrap_or_else(|error| panic!("fixture secret store should open: {error}"));
        let key = account_credential_bundle_key(&self.account_id, generation)
            .unwrap_or_else(|error| panic!("fixture secret key should parse: {error}"));
        store
            .write_secret(&key, serialized)
            .unwrap_or_else(|error| panic!("fixture secret should write: {error}"));
    }

    async fn load(
        &self,
        expected_generation: ActiveCredentialGeneration,
    ) -> Result<PinnedResetAuthority, CredentialAuthorityError> {
        load_reset_credential_authority(
            &self.database_path,
            &self.secret_root,
            &self.account_id,
            expected_generation,
            200,
        )
        .await
    }
}

impl Drop for AuthorityFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root)
            .unwrap_or_else(|error| panic!("fixture root should clean up: {error}"));
    }
}

fn unique_test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "codex-router-reset-{name}-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn recursive_manifest(root: &Path) -> BTreeMap<String, String> {
    let mut manifest = BTreeMap::new();
    collect_manifest(root, root, &mut manifest);
    manifest
}

fn collect_manifest(root: &Path, path: &Path, manifest: &mut BTreeMap<String, String>) {
    let metadata = fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("fixture metadata should load: {error}"));
    let relative_path = path
        .strip_prefix(root)
        .unwrap_or_else(|error| panic!("fixture path should be under root: {error}"));
    let relative = if relative_path.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        relative_path.to_string_lossy().into_owned()
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    };
    #[cfg(not(unix))]
    let mode = 0;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(path)
            .unwrap_or_else(|error| panic!("fixture symlink target should load: {error}"));
        manifest.insert(relative, format!("symlink:{mode:o}:{}", target.display()));
    } else if metadata.is_file() {
        let bytes =
            fs::read(path).unwrap_or_else(|error| panic!("fixture bytes should load: {error}"));
        let hash = Sha256::digest(&bytes);
        manifest.insert(relative, format!("file:{mode:o}:{}:{hash:x}", bytes.len()));
    } else {
        manifest.insert(relative, format!("dir:{mode:o}"));
        let mut children = fs::read_dir(path)
            .unwrap_or_else(|error| panic!("fixture directory should list: {error}"))
            .map(|entry| {
                entry
                    .unwrap_or_else(|error| panic!("fixture entry should load: {error}"))
                    .path()
            })
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            collect_manifest(root, &child, manifest);
        }
    }
}
