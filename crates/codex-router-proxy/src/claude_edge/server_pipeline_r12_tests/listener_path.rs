use super::account_id;
use super::local_router_token;
use super::read_upstream_request;
use super::send_claude_request;
use super::test_database_path;
use crate::server::LoopbackBindAddress;
use crate::server::LoopbackRouterRuntime;
use crate::server::LoopbackRouterRuntimeConfig;
use crate::upstream::ClaudeUpstreamEndpoint;
use crate::upstream::UpstreamEndpoint;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_core::route_profile::WindowKind;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::provider_credential_bundle_key;
use codex_router_secret_store::credential_bundle::CredentialBundle;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::model::CredentialMigrationFailure;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::account_routing_policy::WeeklyQuotaFloorBasisPoints as StateWeeklyFloor;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::AsyncWeeklyQuotaFloorMutationStore;
use codex_router_state::window_observation::WindowObservation;
use codex_router_state::window_observation::WindowObservationProps;
use std::io::Write;
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use tokio::net::TcpListener as TokioTcpListener;

fn runtime_config(
    database_path: &std::path::Path,
    secret_root: &std::path::Path,
) -> LoopbackRouterRuntimeConfig {
    LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new("127.0.0.1", 0)
            .unwrap_or_else(|error| panic!("fixture bind address should be valid: {error}")),
        UpstreamEndpoint::new("http://127.0.0.1:1/v1")
            .unwrap_or_else(|error| panic!("unused fixture upstream should be valid: {error}")),
        database_path.to_path_buf(),
        secret_root.to_path_buf(),
    )
    .with_claude_edge_local_token(
        local_router_token("claude-r12-local", 1),
        Duration::from_secs(180),
    )
}

fn serve_one_request(runtime: LoopbackRouterRuntime) -> String {
    let router_address = runtime.local_addr();
    let server_thread = thread::spawn(move || runtime.serve_http_connections(1));
    let response = send_claude_request(router_address);
    let serve_result = server_thread
        .join()
        .unwrap_or_else(|_error| panic!("fixture server thread should join"));
    assert_eq!(serve_result.unwrap_or(0), 1);
    response
}

fn assert_r12_message(response: &str, expected_message: &str) {
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("fixture Claude response should have a body separator"));
    assert!(
        headers.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    let body: serde_json::Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("Claude selection response should be JSON: {error}"));
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "api_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(expected_message)),
        "{body}"
    );
}

fn fake_claude_upstream_probe() -> (
    ClaudeUpstreamEndpoint,
    std::sync::mpsc::SyncSender<()>,
    thread::JoinHandle<bool>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("fake Claude upstream should bind: {error}"));
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("fake upstream listener should be nonblocking: {error}"));
    let address = listener.local_addr().unwrap_or_else(|error| {
        panic!("fake Claude upstream address should be available: {error}")
    });
    let (probe_start_sender, probe_start_receiver) = std::sync::mpsc::sync_channel(1);
    let probe = thread::spawn(move || {
        probe_start_receiver
            .recv()
            .unwrap_or_else(|error| panic!("fake upstream probe should be started: {error}"));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("fake upstream runtime should build: {error}"));
        runtime.block_on(async move {
            let listener = TokioTcpListener::from_std(listener).unwrap_or_else(|error| {
                panic!("fake upstream listener should become async: {error}")
            });
            match tokio::time::timeout(Duration::from_secs(1), listener.accept()).await {
                Ok(Ok((_stream, _address))) => true,
                Ok(Err(error)) => panic!("fake upstream accept should succeed: {error}"),
                Err(_timeout) => false,
            }
        })
    });
    let endpoint =
        ClaudeUpstreamEndpoint::isolated_debug_override(format!("http://{address}"), true)
            .unwrap_or_else(|error| panic!("isolated fake upstream should be valid: {error}"));
    (endpoint, probe_start_sender, probe)
}

fn assert_fake_upstream_received_no_request(
    probe_start: std::sync::mpsc::SyncSender<()>,
    upstream_probe: thread::JoinHandle<bool>,
) {
    probe_start
        .send(())
        .unwrap_or_else(|error| panic!("fake upstream probe should be released: {error}"));
    assert!(
        !upstream_probe.join().unwrap_or(true),
        "selection failure must not reach the fake upstream"
    );
}

#[test]
fn loopback_claude_no_enabled_accounts_returns_reason_one() {
    let database_path = test_database_path("no-accounts");
    let secret_root = database_path.with_extension("secrets");
    let (endpoint, probe_start, upstream_probe) = fake_claude_upstream_probe();
    let config =
        runtime_config(&database_path, &secret_root).with_debug_claude_upstream_endpoint(endpoint);
    let runtime = LoopbackRouterRuntime::start_for_test(config)
        .unwrap_or_else(|error| panic!("fixture Router should start: {error}"));

    let response = serve_one_request(runtime);
    assert_r12_message(&response, "No Claude account is configured or enabled");
    assert_fake_upstream_received_no_request(probe_start, upstream_probe);
}

async fn add_enabled_account_without_credential(
    database_path: &std::path::Path,
    account_label: &str,
) {
    let account_id = account_id(account_label);
    AsyncSqliteStateStore::open(database_path)
        .await
        .unwrap_or_else(|error| panic!("fixture state should open: {error}"))
        .upsert_account(&AccountRecord::new(
            Provider::Claude,
            account_id,
            account_label,
            AccountStatus::Enabled,
        ))
        .await
        .unwrap_or_else(|error| panic!("fixture account should be stored: {error}"));
}

#[test]
fn loopback_claude_key_unavailable_returns_reason_two() {
    let database_path = test_database_path("key-unavailable");
    let secret_root = database_path.with_extension("secrets");
    let setup_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("fixture setup runtime should build: {error}"));
    setup_runtime.block_on(add_enabled_account_without_credential(
        &database_path,
        "Claude Locked",
    ));
    let file_store = FileSecretStore::open(&secret_root)
        .unwrap_or_else(|error| panic!("fixture file store should open: {error}"));
    let (endpoint, probe_start, upstream_probe) = fake_claude_upstream_probe();
    let runtime = LoopbackRouterRuntime::start(
        runtime_config(&database_path, &secret_root).with_debug_claude_upstream_endpoint(endpoint),
        EncryptedCredentialStore::key_unavailable(file_store),
    )
    .unwrap_or_else(|error| panic!("fixture Router should start with locked credentials: {error}"));

    assert_r12_message(&serve_one_request(runtime), "Keychain key is unreadable");
    assert_fake_upstream_received_no_request(probe_start, upstream_probe);
}

#[test]
fn loopback_claude_migration_incomplete_returns_reason_two() {
    let database_path = test_database_path("migration-incomplete");
    let secret_root = database_path.with_extension("secrets");
    let setup_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("fixture setup runtime should build: {error}"));
    setup_runtime.block_on(add_enabled_account_without_credential(
        &database_path,
        "Claude Migrating",
    ));
    let file_store = FileSecretStore::open(&secret_root)
        .unwrap_or_else(|error| panic!("fixture file store should open: {error}"));
    let (endpoint, probe_start, upstream_probe) = fake_claude_upstream_probe();
    let runtime = LoopbackRouterRuntime::start(
        runtime_config(&database_path, &secret_root).with_debug_claude_upstream_endpoint(endpoint),
        EncryptedCredentialStore::migration_incomplete(
            file_store,
            vec!["Claude Migrating".to_owned()],
            CredentialMigrationFailure::MigrationNotComplete,
        ),
    )
    .unwrap_or_else(|error| {
        panic!("fixture Router should start with incomplete migration: {error}")
    });

    assert_r12_message(
        &serve_one_request(runtime),
        "pooled-credential migration is incomplete",
    );
    assert_fake_upstream_received_no_request(probe_start, upstream_probe);
}

#[test]
fn loopback_claude_selection_failure_returns_account_login_reason() {
    let database_path = test_database_path("needs-login");
    let secret_root = database_path.with_extension("secrets");
    let account_id = account_id("needs-login");
    let setup_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("fixture setup runtime should build: {error}"));
    setup_runtime.block_on(async {
        AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture state should open: {error}"))
            .upsert_account(&AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                "Claude Home",
                AccountStatus::Enabled,
            ))
            .await
            .unwrap_or_else(|error| panic!("fixture account should be stored: {error}"));
    });

    let (endpoint, probe_start, upstream_probe) = fake_claude_upstream_probe();
    let config =
        runtime_config(&database_path, &secret_root).with_debug_claude_upstream_endpoint(endpoint);
    let runtime = LoopbackRouterRuntime::start_for_test(config)
        .unwrap_or_else(|error| panic!("fixture Router should start: {error}"));
    let response = serve_one_request(runtime);
    assert_fake_upstream_received_no_request(probe_start, upstream_probe);
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("fixture Claude response should have a body separator"));
    assert!(
        headers.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    let parsed_body: serde_json::Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("Claude selection response should be JSON: {error}"));
    assert_eq!(parsed_body["type"], "error");
    assert_eq!(parsed_body["error"]["type"], "api_error");
    let message = parsed_body["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("Claude selection message should be present"));
    assert!(message.contains("Claude Home"));
    assert!(message.contains("account login"));
    assert!(!message.contains(account_id.as_str()));
}

#[test]
fn loopback_claude_hard_weekly_floor_returns_reason_five() {
    let database_path = test_database_path("hard-floor");
    let secret_root = database_path.with_extension("secrets");
    let account_id = account_id("hard-floor");
    let setup_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("fixture setup runtime should build: {error}"));
    setup_runtime.block_on(async {
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture state should open: {error}"));
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "Claude Floor",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await
            .unwrap_or_else(|error| panic!("fixture account should be stored: {error}"));
        let mutation_store = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture floor store should open: {error}"));
        mutation_store
            .set_weekly_quota_floor_by_label(
                "Claude Floor",
                Some(StateWeeklyFloor::new(100).unwrap_or_else(|error| {
                    panic!("fixture weekly floor should be valid: {error}")
                })),
            )
            .await
            .unwrap_or_else(|error| panic!("fixture floor should be stored: {error}"));
        mutation_store.close().await;
        for (window_kind, remaining_basis_points) in
            [(WindowKind::FiveHour, 10_000), (WindowKind::Weekly, 0)]
        {
            state
                .record_window_observation(
                    &WindowObservation::new(
                        WindowObservationProps::new(
                            account_id.clone(),
                            window_kind,
                            remaining_basis_points,
                            1_000,
                        )
                        .with_reset_unix_seconds(5_000)
                        .with_fresh_until_unix_seconds(2_000),
                    )
                    .unwrap_or_else(|error| panic!("fixture observation should be valid: {error}")),
                    || 1_100,
                )
                .await
                .unwrap_or_else(|error| panic!("fixture observation should be stored: {error}"));
        }
    });
    let credentials =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root)
            .unwrap_or_else(|error| panic!("fixture encrypted store should open: {error}"));
    let (endpoint, probe_start, upstream_probe) = fake_claude_upstream_probe();
    let config = runtime_config(&database_path, &secret_root)
        .with_quota_clock(1_100, 300)
        .with_debug_claude_upstream_endpoint(endpoint);
    let runtime = LoopbackRouterRuntime::start(config, credentials)
        .unwrap_or_else(|error| panic!("fixture Router should start: {error}"));

    assert_r12_message(
        &serve_one_request(runtime),
        "Claude Floor (at the configured hard quota floor)",
    );
    assert_fake_upstream_received_no_request(probe_start, upstream_probe);
}

#[test]
fn loopback_exhaustion_preserves_provider_attempt_one_usage_limit_response() {
    let database_path = test_database_path("attempt-one-limit");
    let secret_root = database_path.with_extension("secrets");
    let account_id = account_id("attempt-one-limit");
    let credentials =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root)
            .unwrap_or_else(|error| panic!("fixture encrypted store should open: {error}"));
    let credential_bundle = CredentialBundle::new_claude(
        SecretString::new("claude-access-fixture"),
        SecretString::new("claude-refresh-fixture"),
        10_000,
    )
    .unwrap_or_else(|error| panic!("fixture Claude bundle should be valid: {error}"));
    credentials
        .write_secret(
            &provider_credential_bundle_key(Provider::Claude, &account_id, 1)
                .unwrap_or_else(|error| panic!("fixture Claude key should be valid: {error}")),
            &credential_bundle
                .to_secret_string()
                .unwrap_or_else(|error| panic!("fixture Claude bundle should serialize: {error}")),
        )
        .unwrap_or_else(|error| panic!("fixture Claude credential should be stored: {error}"));

    let setup_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("fixture setup runtime should build: {error}"));
    setup_runtime.block_on(async {
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture state should open: {error}"));
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "Claude Home",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await
            .unwrap_or_else(|error| panic!("fixture account should be stored: {error}"));
        for window_kind in [WindowKind::FiveHour, WindowKind::Weekly] {
            state
                .record_window_observation(
                    &WindowObservation::new(
                        WindowObservationProps::new(account_id.clone(), window_kind, 10_000, 1_000)
                            .with_reset_unix_seconds(5_000)
                            .with_fresh_until_unix_seconds(2_000),
                    )
                    .unwrap_or_else(|error| panic!("fixture observation should be valid: {error}")),
                    || 1_100,
                )
                .await
                .unwrap_or_else(|error| panic!("fixture observation should be stored: {error}"));
        }
    });

    let upstream_listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("fixture upstream should bind: {error}"));
    upstream_listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("fixture upstream should be nonblocking: {error}"));
    let upstream_address = upstream_listener
        .local_addr()
        .unwrap_or_else(|error| panic!("fixture upstream address should be available: {error}"));
    let provider_body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"provider usage limit sentinel"}}"#;
    let provider_response = format!(
        "HTTP/1.1 429 Too Many Requests\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\nanthropic-ratelimit-unified-status: rejected\r\nanthropic-ratelimit-unified-representative-claim: five_hour\r\nanthropic-ratelimit-unified-5h-reset: 5000\r\n\r\n{provider_body}",
        provider_body.len()
    );
    let upstream_thread = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("fixture upstream runtime should build: {error}"));
        runtime.block_on(async move {
            let listener = TokioTcpListener::from_std(upstream_listener).unwrap_or_else(|error| {
                panic!("fixture upstream listener should become async: {error}")
            });
            let (stream, _) =
                match tokio::time::timeout(Duration::from_secs(5), listener.accept()).await {
                    Ok(Ok(accepted)) => accepted,
                    Ok(Err(error)) => panic!("fixture upstream should accept: {error}"),
                    Err(_timeout) => return 0,
                };
            let mut stream = stream.into_std().unwrap_or_else(|error| {
                panic!("fixture upstream stream should become blocking: {error}")
            });
            stream.set_nonblocking(false).unwrap_or_else(|error| {
                panic!("fixture upstream stream should be blocking: {error}")
            });
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap_or_else(|error| panic!("fixture upstream timeout should be set: {error}"));
            read_upstream_request(&mut stream);
            stream
                .write_all(provider_response.as_bytes())
                .unwrap_or_else(|error| {
                    panic!("fixture provider response should be written: {error}")
                });
            1
        })
    });

    let config = runtime_config(&database_path, &secret_root)
        .with_quota_clock(1_100, 300)
        .with_debug_claude_upstream_endpoint(
            ClaudeUpstreamEndpoint::isolated_debug_override(
                format!("http://{upstream_address}"),
                true,
            )
            .unwrap_or_else(|error| {
                panic!("isolated fixture Claude upstream should be valid: {error}")
            }),
        );
    let runtime = LoopbackRouterRuntime::start(config, credentials)
        .unwrap_or_else(|error| panic!("fixture Router should start: {error}"));

    let response = serve_one_request(runtime);
    let upstream_request_count = upstream_thread
        .join()
        .unwrap_or_else(|_error| panic!("fixture upstream thread should join"));
    assert_eq!(upstream_request_count, 1);
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("fixture Claude response should have a body separator"));
    assert!(
        headers.starts_with("HTTP/1.1 429 Too Many Requests"),
        "{response}"
    );
    assert!(
        headers.contains("anthropic-ratelimit-unified-status: rejected"),
        "{headers}"
    );
    assert_eq!(body, provider_body);
}
