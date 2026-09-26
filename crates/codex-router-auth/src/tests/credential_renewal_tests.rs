use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_waiter_does_not_cancel_provider_rotation_or_release_account_lock() {
    let temp_dir = AuthTestTempDir::new("cancelled-refresh-waiter");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
    let account_id = account_id("cancelled-waiter-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(account_id.clone(), "cancelled", AccountStatus::Enabled)
                    .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("old-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let (entered_sender, entered_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let refresh_client = HeldRefreshClient {
        calls: Arc::new(AtomicUsize::new(0)),
        entered_sender,
        release_receiver: Arc::new(Mutex::new(release_receiver)),
    };
    let first_resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        refresh_client.clone(),
        Some(1_000),
    );
    let first_account = account_id.clone();
    let first_waiter = tokio::spawn(async move {
        first_resolver
            .resolve_provider_credentials(&first_account)
            .await
    });
    must_ok(
        tokio::task::spawn_blocking(move || {
            entered_receiver.recv_timeout(std::time::Duration::from_secs(2))
        })
        .await,
    )
    .expect("provider request should start");
    first_waiter.abort();
    assert!(first_waiter.await.is_err());

    let second_resolver = AsyncRouterCredentialResolver::new(
        must_ok(AsyncSqliteStateStore::open(&database_path).await),
        secrets,
        refresh_client.clone(),
        Some(1_000),
    );
    let second_account = account_id.clone();
    let second_waiter = tokio::spawn(async move {
        second_resolver
            .resolve_provider_credentials(&second_account)
            .await
    });
    assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
    must_ok(release_sender.send(()));
    let resolved = must_ok(
        must_ok(tokio::time::timeout(std::time::Duration::from_secs(2), second_waiter).await)
            .expect("second waiter should finish"),
    );
    assert_eq!(resolved.credential_generation(), 2);
    assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
    let active = must_ok(state.load_account(&account_id).await).expect("account should remain");
    assert_eq!(active.active_credential_generation(), Some(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn separate_process_resolvers_use_one_rotating_refresh() {
    if let Some(root) = std::env::var_os("CODEX_ROUTER_CROSS_PROCESS_REFRESH_ROOT") {
        let root = PathBuf::from(root);
        fs::write(root.join("child-ready"), b"ready").expect("child should report ready");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !root.join("start").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "child start timed out"
            );
            thread::yield_now();
        }
        let state = must_ok(AsyncSqliteStateStore::open(&root.join("state.sqlite")).await);
        let secrets = must_ok(FileSecretStore::open(root.join("secrets")));
        let resolver = AsyncRouterCredentialResolver::new(
            state,
            secrets,
            CrossProcessRefreshClient { root },
            Some(1_000),
        );
        let resolved = must_ok(
            resolver
                .resolve_provider_credentials(&account_id("cross-process-account"))
                .await,
        );
        assert_eq!(resolved.credential_generation(), 2);
        return;
    }

    let temp_dir = AuthTestTempDir::new("cross-process-refresh");
    let root = temp_dir.path();
    let state = must_ok(AsyncSqliteStateStore::open(&root.join("state.sqlite")).await);
    let secrets = must_ok(FileSecretStore::open(root.join("secrets")));
    let account_id = account_id("cross-process-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(account_id.clone(), "cross process", AccountStatus::Enabled)
                    .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("old-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let mut child = must_ok(
        Command::new(std::env::current_exe().expect("test binary"))
            .arg("--exact")
            .arg("tests::credential_renewal_tests::separate_process_resolvers_use_one_rotating_refresh")
            .env("CODEX_ROUTER_CROSS_PROCESS_REFRESH_ROOT", root)
            .stdout(std::process::Stdio::null())
            .spawn(),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !root.join("child-ready").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child readiness timed out"
        );
        tokio::task::yield_now().await;
    }
    let resolver = AsyncRouterCredentialResolver::new(
        state,
        secrets,
        CrossProcessRefreshClient {
            root: root.to_path_buf(),
        },
        Some(1_000),
    );
    must_ok(fs::write(root.join("start"), b"start"));
    let resolved = must_ok(resolver.resolve_provider_credentials(&account_id).await);
    assert_eq!(resolved.credential_generation(), 2);
    assert!(
        must_ok(child.wait()).success(),
        "child resolver should finish"
    );
    let provider_calls = must_ok(fs::read_dir(root))
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("provider-call-")
        })
        .count();
    assert_eq!(provider_calls, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_waiter_keeps_lock_until_blocking_secret_write_and_activation_finish() {
    let temp_dir = AuthTestTempDir::new("held-secret-write");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let file_secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
    let account_id = account_id("held-secret-write-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(account_id.clone(), "held", AccountStatus::Enabled)
                    .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        file_secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("old-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let (entered_sender, entered_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let secrets = HeldSecretWriteStore {
        inner: file_secrets,
        entered_sender,
        release_receiver: Arc::new(Mutex::new(release_receiver)),
    };
    let refresh_client = RecordingRefreshClient::new_for_account(
        "held-secret-write-account",
        "old-refresh-canary",
        AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    );
    let first_resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        refresh_client.clone(),
        Some(1_000),
    );
    let first_account = account_id.clone();
    let first_waiter = tokio::spawn(async move {
        first_resolver
            .resolve_provider_credentials(&first_account)
            .await
    });
    must_ok(
        tokio::task::spawn_blocking(move || entered_receiver.recv_timeout(Duration::from_secs(2)))
            .await,
    )
    .expect("successor write should start");
    first_waiter.abort();
    assert!(first_waiter.await.is_err());
    let second_resolver = AsyncRouterCredentialResolver::new(
        must_ok(AsyncSqliteStateStore::open(&database_path).await),
        secrets,
        refresh_client.clone(),
        Some(1_000),
    );
    let second_account = account_id.clone();
    let second_waiter = tokio::spawn(async move {
        second_resolver
            .resolve_provider_credentials(&second_account)
            .await
    });
    let still_active = must_ok(state.load_account(&account_id).await).expect("account");
    assert_eq!(still_active.active_credential_generation(), Some(1));
    let claim = must_ok(state.load_credential_maintenance(&account_id).await).expect("claim");
    assert_eq!(claim.state, CredentialMaintenanceState::InProgress);
    assert_eq!(refresh_client.calls(), 1);
    must_ok(release_sender.send(()));
    let resolved = must_ok(
        must_ok(tokio::time::timeout(Duration::from_secs(2), second_waiter).await)
            .expect("second resolver should finish"),
    );
    assert_eq!(resolved.credential_generation(), 2);
    assert_eq!(refresh_client.calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_secret_write_failure_retries_commit_without_second_provider_use() {
    let temp_dir = AuthTestTempDir::new("transient-secret-write");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let file_secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
    let account_id = account_id("transient-write-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(account_id.clone(), "write", AccountStatus::Enabled)
                    .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        file_secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("old-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let secrets = FailOnceSecretWriteStore {
        inner: file_secrets.clone(),
        failed_once: Arc::new(AtomicBool::new(false)),
    };
    let refresh_client = RecordingRefreshClient::new_for_account(
        "transient-write-account",
        "old-refresh-canary",
        AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    );
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        refresh_client.clone(),
        Some(1_000),
    );
    let resolved = must_ok(
        tokio::time::timeout(
            Duration::from_secs(2),
            resolver.resolve_provider_credentials(&account_id),
        )
        .await,
    );
    let resolved = must_ok(resolved);
    assert_eq!(resolved.credential_generation(), 2);
    assert!(secrets.failed_once.load(Ordering::SeqCst));
    assert_eq!(refresh_client.calls(), 1);
    let active = must_ok(state.load_account(&account_id).await).expect("account");
    assert_eq!(active.active_credential_generation(), Some(2));
    let successor_key = must_ok(account_credential_bundle_key(&account_id, 2));
    assert!(file_secrets.read_secret(&successor_key).is_ok());
}

#[tokio::test]
async fn inaccessible_successor_slot_records_local_retry_without_provider_use() {
    use codex_router_state::credential_maintenance::CredentialFailureClass;

    let temp_dir = AuthTestTempDir::new("inaccessible-successor-slot");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let file_secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
    let account_id = account_id("inaccessible-successor-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(account_id.clone(), "inaccessible", AccountStatus::Enabled)
                    .with_active_credential_generation(1),
            )
            .await,
    );
    let key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        file_secrets.write_secret(
            &key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("refresh-token-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let refresh_client = RecordingRefreshClient::new_for_account(
        "inaccessible-successor-account",
        "refresh-token-canary",
        AccountCredentialBundle::imported_codex_auth("unused-access", None),
    );
    let unreadable_secrets = UnreadableSuccessorSlotStore {
        inner: file_secrets,
    };
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        unreadable_secrets.clone(),
        refresh_client.clone(),
        Some(1_000),
    );

    assert_eq!(
        resolver.resolve_provider_credentials(&account_id).await,
        Err(CredentialResolverError::SecretUnavailable)
    );
    assert_eq!(refresh_client.calls(), 0);
    let health = must_ok(state.load_credential_maintenance(&account_id).await).expect("health");
    assert_eq!(health.state, CredentialMaintenanceState::Retrying);
    assert_eq!(
        health.failure_class,
        Some(CredentialFailureClass::LocalPersistence)
    );
    assert_eq!(health.next_attempt_unix_seconds, Some(1_060));
    assert_eq!(
        must_ok(
            resolver
                .next_maintenance_due_unix_seconds(&account_id)
                .await
        ),
        Some(1_060)
    );
    let due_resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        unreadable_secrets,
        refresh_client.clone(),
        Some(1_060),
    );
    assert_eq!(
        due_resolver.resolve_provider_credentials(&account_id).await,
        Err(CredentialResolverError::SecretUnavailable)
    );
    let after_retry =
        must_ok(state.load_credential_maintenance(&account_id).await).expect("retry health");
    assert_eq!(after_retry.next_attempt_unix_seconds, Some(1_180));
    assert_eq!(refresh_client.calls(), 0);
}

#[derive(Clone)]
struct UnreadableSuccessorSlotStore {
    inner: FileSecretStore,
}

impl SecretStore for UnreadableSuccessorSlotStore {
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        self.inner.write_secret(key, secret)
    }

    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        if key.as_str().ends_with(".2") {
            return Err(SecretStoreError::Filesystem {
                path: PathBuf::from("fixture-inaccessible-slot"),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            });
        }
        self.inner.read_secret(key)
    }
}

#[derive(Clone)]
struct FailOnceSecretWriteStore {
    inner: FileSecretStore,
    failed_once: Arc<AtomicBool>,
}

impl SecretStore for FailOnceSecretWriteStore {
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        if key.as_str().ends_with(".2") && !self.failed_once.swap(true, Ordering::SeqCst) {
            return Err(SecretStoreError::Filesystem {
                path: PathBuf::from("fixture-successor-write"),
                source: std::io::Error::other("fixture write failed"),
            });
        }
        self.inner.write_secret(key, secret)
    }

    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        self.inner.read_secret(key)
    }
}

#[derive(Clone)]
struct HeldSecretWriteStore {
    inner: FileSecretStore,
    entered_sender: std::sync::mpsc::Sender<()>,
    release_receiver: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
}

impl SecretStore for HeldSecretWriteStore {
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        if key.as_str().ends_with(".2") {
            self.entered_sender
                .send(())
                .expect("write should report entry");
            self.release_receiver
                .lock()
                .expect("release lock")
                .recv_timeout(Duration::from_secs(2))
                .expect("write should be released");
        }
        self.inner.write_secret(key, secret)
    }

    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        self.inner.read_secret(key)
    }
}

#[derive(Clone)]
struct CrossProcessRefreshClient {
    root: PathBuf,
}

impl CredentialRefreshClient for CrossProcessRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, crate::resolver::CredentialRefreshFailure> {
        fs::write(
            self.root
                .join(format!("provider-call-{}", std::process::id())),
            b"used",
        )
        .expect("provider call should be recorded");
        thread::sleep(Duration::from_millis(100));
        Ok(AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000))
    }
}

#[derive(Clone)]
struct HeldRefreshClient {
    calls: Arc<AtomicUsize>,
    entered_sender: std::sync::mpsc::Sender<()>,
    release_receiver: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
}

impl CredentialRefreshClient for HeldRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, crate::resolver::CredentialRefreshFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered_sender.send(()).expect("entry should report");
        self.release_receiver
            .lock()
            .expect("release lock")
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("provider should be released");
        Ok(AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000))
    }
}
