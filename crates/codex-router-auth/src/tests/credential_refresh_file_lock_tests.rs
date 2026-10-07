use super::credential_refresh_supervision_tests::recording_refresh_client;
use super::credential_refresh_supervision_tests::seed_openai_account;
use super::*;
use crate::resolver::CredentialRefreshTaskSupervisor;
use codex_router_core::provider::Provider;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_file_lock_wait_finishes_acquire_only_without_claim_or_leaked_lock() {
    if let Some(root) = std::env::var_os("CODEX_ROUTER_RENEWAL_LOCK_ROOT") {
        let root = PathBuf::from(root);
        let _lock = must_ok(
            codex_router_secret_store::account_credential_lock::AccountCredentialLock::acquire(
                &root.join("state.sqlite"),
                &account_id("file-lock-waiter-account"),
            ),
        );
        must_ok(fs::write(root.join("child-lock-ready"), b"held"));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !root.join("release-child-lock").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "lock release timed out"
            );
            thread::yield_now();
        }
        return;
    }

    let temp_dir = AuthTestTempDir::new("cancel-file-lock-waiter");
    let root = temp_dir.path();
    let database_path = root.join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            root.join("secrets"),
        ),
    );
    let locked_account_id = account_id("file-lock-waiter-account");
    seed_openai_account(
        &state,
        &secrets,
        &locked_account_id,
        900,
        "refresh-token-canary",
    )
    .await;
    let mut lock_holder = must_ok(
        Command::new(std::env::current_exe().expect("test binary"))
            .arg("--exact")
            .arg("tests::credential_refresh_file_lock_tests::canceled_file_lock_wait_finishes_acquire_only_without_claim_or_leaked_lock")
            .env("CODEX_ROUTER_RENEWAL_LOCK_ROOT", root)
            .stdout(std::process::Stdio::null())
            .spawn(),
    );
    let child_ready = root.join("child-lock-ready");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !child_ready.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child lock acquisition timed out"
        );
        tokio::task::yield_now().await;
    }

    let refresh_tasks = CredentialRefreshTaskSupervisor::new();
    let (task_pending_sender, mut task_pending_receiver) = tokio::sync::mpsc::unbounded_channel();
    refresh_tasks.set_test_task_pending_observer(task_pending_sender);
    let (lock_acquired_sender, lock_acquired_receiver) = std::sync::mpsc::channel();
    let refresh_client = recording_refresh_client(&locked_account_id);
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        refresh_client.clone(),
        Some(1_000),
    )
    .with_refresh_task_supervisor(refresh_tasks.clone())
    .with_test_file_lock_acquired_observer(lock_acquired_sender);
    let resolution = tokio::spawn(async move {
        resolver
            .resolve_provider_credentials(&account_id("file-lock-waiter-account"), Provider::Openai)
            .await
    });
    assert!(
        task_pending_receiver.recv().await.is_some(),
        "the tracked renewal is pending on the held cross-process account lock"
    );

    refresh_tasks.close_admission();
    assert_eq!(
        must_ok(tokio::time::timeout(Duration::from_secs(2), resolution).await)
            .expect("canceled lock waiter returns"),
        Err(CredentialResolverError::RenewalAdmissionClosed)
    );
    assert_eq!(refresh_client.calls(), 0);
    assert_eq!(
        must_ok(state.load_account(&locked_account_id).await)
            .expect("account remains stored")
            .active_credential_generation(),
        Some(1)
    );
    assert_eq!(
        must_ok(state.load_credential_maintenance(&locked_account_id).await),
        None
    );

    must_ok(fs::write(root.join("release-child-lock"), b"release"));
    let lock_holder_status = must_ok(must_ok(
        tokio::task::spawn_blocking(move || lock_holder.wait()).await,
    ));
    assert!(
        lock_holder_status.success(),
        "the cross-process lock holder exits"
    );
    must_ok(
        tokio::task::spawn_blocking(move || {
            lock_acquired_receiver.recv_timeout(Duration::from_secs(2))
        })
        .await,
    )
    .expect("detached acquire-only closure finishes after release");
    let lock_probe_path = database_path.clone();
    let lock_probe_account = account_id("file-lock-waiter-account");
    let lock_probe = tokio::task::spawn_blocking(move || {
        codex_router_secret_store::account_credential_lock::AccountCredentialLock::acquire(
            &lock_probe_path,
            &lock_probe_account,
        )
    });
    assert!(
        must_ok(tokio::time::timeout(Duration::from_secs(2), lock_probe).await)
            .expect("file lock probe completes")
            .is_ok(),
        "the abandoned acquisition released its file guard"
    );
    assert_eq!(refresh_client.calls(), 0);
    assert_eq!(
        must_ok(state.load_account(&locked_account_id).await)
            .expect("account remains stored")
            .active_credential_generation(),
        Some(1)
    );
    assert_eq!(
        must_ok(state.load_credential_maintenance(&locked_account_id).await),
        None
    );
    let successor_key = must_ok(openai_account_credential_bundle_key(&locked_account_id, 2));
    assert!(secrets.read_secret(&successor_key).is_err());
}
