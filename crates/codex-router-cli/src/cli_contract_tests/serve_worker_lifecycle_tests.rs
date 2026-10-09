use super::*;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use std::sync::mpsc;
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
struct HeldServeUpkeepRefreshClient {
    started_for_upkeep: tokio::sync::mpsc::UnboundedSender<()>,
    started_for_test: tokio::sync::mpsc::UnboundedSender<()>,
    release_receiver: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl CredentialRefreshClient for HeldServeUpkeepRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        self.started_for_upkeep
            .send(())
            .expect("Serve upkeep startup should observe the active refresh");
        self.started_for_test
            .send(())
            .expect("test should observe the active refresh");
        self.release_receiver
            .lock()
            .expect("release receiver lock")
            .recv_timeout(Duration::from_secs(5))
            .expect("test should release the active refresh");
        Ok(AccountCredentialBundle::imported_codex_auth(
            "serve-upkeep-replacement-access",
            Some("serve-upkeep-replacement-refresh".to_owned()),
        )
        .with_expires_unix_seconds(10_000))
    }
}

fn held_serve_upkeep_fixture(
    test_name: &str,
) -> (
    TestRoot,
    PathBuf,
    PathBuf,
    AccountId,
    EncryptedCredentialStore,
) {
    let test_root = TestRoot::new(test_name);
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("serve-held-upkeep");
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "serve held upkeep",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1),
    ));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let expired_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "serve-upkeep-expired-access",
            Some("serve-upkeep-original-refresh".to_owned()),
        )
        .with_expires_unix_seconds(1_100)
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&active_key, &expired_bundle));
    drop(state);
    (test_root, state_path, secret_root, account_id, secrets)
}

fn serve_command_for_worker_lifecycle(
    state_path: &Path,
    secret_root: &Path,
    port: u16,
    enable_background_quota_refresh: bool,
) -> cli_argument_parsing::ServeCommand {
    let mut arguments = vec![
        OsString::from("serve"),
        OsString::from("--listen-host"),
        OsString::from("127.0.0.1"),
        OsString::from("--port"),
        OsString::from(port.to_string()),
        OsString::from("--state-db"),
        state_path.as_os_str().to_os_string(),
        OsString::from("--secret-root"),
        secret_root.as_os_str().to_os_string(),
        OsString::from("--upstream-base-url"),
        OsString::from("http://127.0.0.1:1/v1"),
        OsString::from("--now-unix-seconds"),
        OsString::from("1000"),
        OsString::from("--max-connections"),
        OsString::from("1"),
    ];
    if !enable_background_quota_refresh {
        arguments.push(OsString::from("--disable-background-quota-refresh"));
    }
    let CliCommand::Serve(command) = must_ok(CliCommand::parse(arguments)) else {
        panic!("Serve arguments should parse as a Serve command");
    };
    assert_eq!(
        command.background_quota_refresh_enabled,
        enable_background_quota_refresh
    );
    command
}

fn blocked_serve_upkeep_client() -> (
    HeldServeUpkeepRefreshClient,
    tokio::sync::mpsc::UnboundedReceiver<()>,
    tokio::sync::mpsc::UnboundedReceiver<()>,
    mpsc::Sender<()>,
) {
    let (started_for_upkeep, upkeep_entry_receiver) = tokio::sync::mpsc::unbounded_channel();
    let (started_for_test, test_entry_receiver) = tokio::sync::mpsc::unbounded_channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let refresh_client = HeldServeUpkeepRefreshClient {
        started_for_upkeep,
        started_for_test,
        release_receiver: Arc::new(Mutex::new(release_receiver)),
    };
    (
        refresh_client,
        test_entry_receiver,
        upkeep_entry_receiver,
        release_sender,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_quota_start_failure_joins_active_upkeep_and_preserves_error_output() {
    let (_test_root, state_path, secret_root, _account_id, secrets) =
        held_serve_upkeep_fixture("serve-quota-start-failure-drain");
    let router_port = reserve_loopback_port();
    let command = serve_command_for_worker_lifecycle(&state_path, &secret_root, router_port, true);
    let (refresh_client, mut test_entry_receiver, mut upkeep_entry_receiver, release_sender) =
        blocked_serve_upkeep_client();
    let mut serve_task = tokio::spawn(async move {
        let mut stdout = Vec::new();
        let result = run_serve_command_with_worker_starts_and_token_reload_observer(
            &mut stdout,
            command,
            secrets,
            move |state_db, credential_store, refresh_tasks| async move {
                let worker =
                    crate::credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock(
                        state_db,
                        credential_store,
                        refresh_tasks,
                        refresh_client,
                        || 1_000,
                    )
                    .await?;
                tokio::time::timeout(Duration::from_secs(2), upkeep_entry_receiver.recv())
                    .await
                    .expect("upkeep should enter the active refresh before returning")
                    .expect("upkeep should send its active refresh entry");
                Ok(worker)
            },
            |_state_db, _secret_root, _credential_store, _base_url, _interval, _notifier, _refresh_tasks| async {
                Err(QuotaRefreshError::BackgroundWorkerInitialization(
                    std::io::Error::other("fixture quota startup failure"),
                ))
            },
            |_generation| {},
        )
        .await;
        (result, stdout)
    });

    tokio::time::timeout(Duration::from_secs(2), test_entry_receiver.recv())
        .await
        .expect("fixture upkeep refresh should enter")
        .expect("fixture upkeep refresh should signal entry");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut serve_task)
            .await
            .is_err()
    );
    must_ok(release_sender.send(()));

    let (serve_result, stdout) = serve_task
        .await
        .unwrap_or_else(|error| panic!("Serve task should join after refresh release: {error}"));
    let error = serve_result.expect_err("quota startup failure should be preserved");
    assert!(matches!(
        error,
        CliError::Quota(QuotaCommandError::BackgroundWorkerInitialization(_))
    ));
    assert!(error.to_string().contains("fixture quota startup failure"));
    assert!(
        String::from_utf8_lossy(&stdout).contains(&format!("listening: 127.0.0.1:{router_port}"))
    );
    assert!(TcpStream::connect(("127.0.0.1", router_port)).is_err());
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::time::sleep(Duration::from_millis(1)),
    )
    .await
    .expect("caller Tokio runtime should remain usable after partial-start cleanup");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_protocol_error_joins_active_upkeep_and_preserves_runtime_error() {
    let (_test_root, state_path, secret_root, _account_id, secrets) =
        held_serve_upkeep_fixture("serve-runtime-error-drain");
    let router_port = reserve_loopback_port();
    let command = serve_command_for_worker_lifecycle(&state_path, &secret_root, router_port, false);
    let (refresh_client, mut test_entry_receiver, mut upkeep_entry_receiver, release_sender) =
        blocked_serve_upkeep_client();
    let mut serve_task = tokio::spawn(async move {
        let mut stdout = Vec::new();
        let result = run_serve_command_with_upkeep_start_and_token_reload_observer(
            &mut stdout,
            command,
            secrets,
            move |state_db, credential_store, refresh_tasks| async move {
                let worker =
                    crate::credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock(
                        state_db,
                        credential_store,
                        refresh_tasks,
                        refresh_client,
                        || 1_000,
                    )
                    .await?;
                tokio::time::timeout(Duration::from_secs(2), upkeep_entry_receiver.recv())
                    .await
                    .expect("upkeep should enter the active refresh before Serve")
                    .expect("upkeep should send its active refresh entry");
                Ok(worker)
            },
            |_generation| {},
        )
        .await;
        (result, stdout)
    });

    tokio::time::timeout(Duration::from_secs(2), test_entry_receiver.recv())
        .await
        .expect("fixture upkeep refresh should enter")
        .expect("fixture upkeep refresh should signal entry");
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", router_port))
        .await
        .unwrap_or_else(|error| {
            panic!("Serve listener should accept the malformed probe: {error}")
        });
    client
        .write_all(b"\x16\x03\x01not-http\r\n\r\n")
        .await
        .unwrap_or_else(|error| panic!("malformed request should write: {error}"));
    client
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("malformed request should half-close: {error}"));

    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut serve_task)
            .await
            .is_err()
    );
    must_ok(release_sender.send(()));

    let (serve_result, stdout) = serve_task
        .await
        .unwrap_or_else(|error| panic!("Serve task should join after refresh release: {error}"));
    assert!(matches!(
        serve_result,
        Err(CliError::Runtime(
            codex_router_proxy::server::LoopbackRouterRuntimeError::HyperConnection(_)
        ))
    ));
    assert!(
        String::from_utf8_lossy(&stdout).contains(&format!("listening: 127.0.0.1:{router_port}"))
    );
    assert!(TcpStream::connect(("127.0.0.1", router_port)).is_err());
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::time::sleep(Duration::from_millis(1)),
    )
    .await
    .expect("caller Tokio runtime should remain usable after Serve error cleanup");
}
