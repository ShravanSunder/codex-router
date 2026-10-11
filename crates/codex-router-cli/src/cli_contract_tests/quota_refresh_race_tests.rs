use super::*;
use crate::quota::QuotaRefreshSchedule;

struct SnapshotRenewalCredentialResolver {
    state_path: PathBuf,
    trigger_account_id: AccountId,
    renewed_account_id: AccountId,
}

impl AsyncProviderCredentialResolver for SnapshotRenewalCredentialResolver {
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        if account_id == &self.trigger_account_id {
            let state = AsyncSqliteStateStore::open(&self.state_path)
                .await
                .expect("state should open during the first account resolution");
            state
                .activate_account_credential_generation_and_invalidate_quota(
                    &self.renewed_account_id,
                    2,
                    AccountStatus::Enabled,
                )
                .await
                .expect("renewing the later account should invalidate its old quota");
            state
                .close()
                .await
                .expect("state should close after the concurrent renewal");
            return Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                SecretString::new("trigger-account-generation-one-token"),
                1,
            ));
        }

        if account_id == &self.renewed_account_id {
            return Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                SecretString::new("renewed-account-generation-two-token"),
                2,
            ));
        }

        Err(CredentialResolverError::AccountUnavailable)
    }
}

struct FixedGenerationCredentialResolver {
    access_token: &'static str,
    credential_generation: u64,
}

impl AsyncProviderCredentialResolver for FixedGenerationCredentialResolver {
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            SecretString::new(self.access_token),
            self.credential_generation,
        ))
    }
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;

    let mut request_bytes = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        let bytes_read = stream
            .read(&mut byte)
            .await
            .unwrap_or_else(|error| panic!("loopback quota provider should read: {error}"));
        if bytes_read == 0 {
            break;
        }
        request_bytes.push(byte[0]);
        if request_bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&request_bytes).into_owned()
}

async fn write_http_response(stream: &mut tokio::net::TcpStream, body: &str) {
    use tokio::io::AsyncWriteExt;

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("loopback quota provider should respond: {error}"));
}

fn quota_provider_body(is_usage_request: bool) -> &'static str {
    if is_usage_request {
        r#"{
            "rate_limit": {
                "primary_window": {
                    "used_percent": 20,
                    "reset_at": 8000,
                    "limit_window_seconds": 18000
                },
                "secondary_window": {
                    "used_percent": 30,
                    "reset_at": 9000,
                    "limit_window_seconds": 604800
                }
            },
            "additional_rate_limits": [],
            "reset_credits": {"available": 1}
        }"#
    } else {
        r#"{"reset_credits":{"available":1}}"#
    }
}

fn is_quota_provider_request(request: &str) -> (bool, bool) {
    let is_usage_request = request.starts_with("GET /api/codex/usage HTTP/1.1\r\n");
    let is_reset_request =
        request.starts_with("GET /api/codex/rate-limit-reset-credits HTTP/1.1\r\n");
    assert!(
        is_usage_request || is_reset_request,
        "unexpected loopback quota request: {request:?}"
    );
    (is_usage_request, is_reset_request)
}

#[tokio::test]
async fn generation_renewal_between_account_snapshot_and_resolution_refreshes_current_generation() {
    let test_root = TestRoot::new("quota-refresh-resolve-after-snapshot");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&state_path).await);
    let trigger_account_id = account_id("acct_a_generation_renewal_trigger");
    let renewed_account_id = account_id("acct_b_generation_renewed");
    for (account_id, label) in [
        (&trigger_account_id, "generation-renewal-trigger"),
        (&renewed_account_id, "generation-renewed"),
    ] {
        must_ok(
            state
                .upsert_account(
                    &AccountRecord::new(
                        codex_router_core::provider::Provider::Openai,
                        account_id.clone(),
                        label,
                        AccountStatus::Enabled,
                    )
                    .with_active_credential_generation(1),
                )
                .await,
        );
    }

    let listener = must_ok(tokio::net::TcpListener::bind("127.0.0.1:0").await);
    let address = must_ok(listener.local_addr());
    let stop_server = Arc::new(tokio::sync::Notify::new());
    let server_stop = Arc::clone(&stop_server);
    let provider_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        loop {
            tokio::select! {
                incoming = listener.accept() => {
                    let (mut stream, _peer_address) = incoming
                        .unwrap_or_else(|error| panic!("loopback quota provider should accept: {error}"));
                    let request = read_http_request(&mut stream).await;
                    let (is_usage_request, _is_reset_request) = is_quota_provider_request(&request);
                    let lowercase_request = request.to_ascii_lowercase();
                    assert!(
                        lowercase_request.contains("authorization: bearer trigger-account-generation-one-token\r\n")
                            || lowercase_request.contains("authorization: bearer renewed-account-generation-two-token\r\n"),
                        "only the resolved account token may reach the provider: {request:?}"
                    );
                    requests.push(request);
                    write_http_response(&mut stream, quota_provider_body(is_usage_request)).await;
                }
                () = server_stop.notified() => return requests,
            }
        }
    });

    let resolver = Arc::new(SnapshotRenewalCredentialResolver {
        state_path: state_path.clone(),
        trigger_account_id,
        renewed_account_id: renewed_account_id.clone(),
    });
    let secret_root = test_root.path().join("unused-secrets");
    let provider = must_ok(HttpQuotaRefreshProvider::new());
    let refresh_state_path = state_path.clone();
    let refresh_resolver = Arc::clone(&resolver);
    let refresh = tokio::spawn(async move {
        let mut output = Vec::new();
        let result = refresh_quota_store_paths_with_dependencies_async(
            &mut output,
            &refresh_state_path,
            &secret_root,
            format!("http://{address}"),
            refresh_resolver.as_ref(),
            &provider,
            1_100,
        )
        .await;
        (result, output)
    });

    let (refresh_result, output) = refresh
        .await
        .unwrap_or_else(|error| panic!("refresh task should complete normally: {error}"));
    stop_server.notify_one();
    let requests = provider_server
        .await
        .unwrap_or_else(|error| panic!("loopback provider task should complete: {error}"));

    must_ok(refresh_result);
    assert_eq!(
        requests.len(),
        8,
        "both accounts should refresh both route bands after the first resolution renews the later account"
    );
    for token in [
        "trigger-account-generation-one-token",
        "renewed-account-generation-two-token",
    ] {
        assert_eq!(
            requests
                .iter()
                .filter(|request| request
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {token}\r\n")))
                .count(),
            4,
            "each resolved account should complete one usage and reset-credit read per route band"
        );
    }
    assert_eq!(
        std::str::from_utf8(&output).expect("refresh output should be UTF-8"),
        "refreshed: 4\n"
    );

    let renewed_observation = state
        .load_account_credit_observation(&renewed_account_id)
        .await
        .expect("renewed account observation should read")
        .expect("the current-generation Responses read should commit");
    assert_eq!(renewed_observation.credential_generation(), 2);
    assert_eq!(renewed_observation.latest_started_attempt(), 1);
    assert_eq!(renewed_observation.committed_attempt(), Some(1));
}

#[tokio::test]
async fn generation_replacement_during_provider_read_rejects_stale_commit_and_remaining_bands() {
    let test_root = TestRoot::new("quota-refresh-generation-replaced-inflight");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&state_path).await);
    let account_id = account_id("acct_inflight_generation_replacement");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "inflight-generation-replacement",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );

    let listener = must_ok(tokio::net::TcpListener::bind("127.0.0.1:0").await);
    let address = must_ok(listener.local_addr());
    let stop_server = Arc::new(tokio::sync::Notify::new());
    let server_stop = Arc::clone(&stop_server);
    let usage_request_started = Arc::new(tokio::sync::Notify::new());
    let server_usage_request_started = Arc::clone(&usage_request_started);
    let release_usage_response = Arc::new(tokio::sync::Notify::new());
    let server_release_usage_response = Arc::clone(&release_usage_response);
    let provider_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut held_first_usage_response = false;
        loop {
            tokio::select! {
                incoming = listener.accept() => {
                    let (mut stream, _peer_address) = incoming
                        .unwrap_or_else(|error| panic!("loopback quota provider should accept: {error}"));
                    let request = read_http_request(&mut stream).await;
                    let (is_usage_request, _is_reset_request) = is_quota_provider_request(&request);
                    assert!(
                        request.to_ascii_lowercase().contains("authorization: bearer generation-one-token\r\n"),
                        "the in-flight provider read must use the resolved generation-one token: {request:?}"
                    );
                    requests.push(request);
                    if is_usage_request && !held_first_usage_response {
                        held_first_usage_response = true;
                        server_usage_request_started.notify_one();
                        server_release_usage_response.notified().await;
                    }
                    write_http_response(&mut stream, quota_provider_body(is_usage_request)).await;
                }
                () = server_stop.notified() => return requests,
            }
        }
    });

    let resolver = Arc::new(FixedGenerationCredentialResolver {
        access_token: "generation-one-token",
        credential_generation: 1,
    });
    let observer = Arc::new(RecordingWeeklyFloorObserver::default());
    let refresh_observer = Arc::clone(&observer);
    let secret_root = test_root.path().join("unused-secrets");
    let provider = must_ok(HttpQuotaRefreshProvider::new());
    let refresh_state_path = state_path.clone();
    let refresh_resolver = Arc::clone(&resolver);
    let mut refresh = tokio::spawn(async move {
        let mut output = Vec::new();
        let result = refresh_quota_store_paths_with_dependencies_and_floor_notifier_async(
            &mut output,
            &refresh_state_path,
            &secret_root,
            format!("http://{address}"),
            refresh_resolver.as_ref(),
            &provider,
            QuotaRefreshObservationContext {
                observed_unix_seconds: 1_100,
                schedule: QuotaRefreshSchedule::Background {
                    interval_seconds: 180,
                },
                weekly_floor_observer: Some(refresh_observer.as_ref()),
            },
        )
        .await;
        (result, output)
    });

    let usage_request_started = usage_request_started.notified();
    tokio::pin!(usage_request_started);
    tokio::select! {
        () = &mut usage_request_started => {}
        result = &mut refresh => panic!(
            "refresh should be blocked on the generation-one Responses read: {result:?}"
        ),
    }

    must_ok(
        state
            .activate_account_credential_generation_and_invalidate_quota(
                &account_id,
                2,
                AccountStatus::Enabled,
            )
            .await,
    );
    release_usage_response.notify_one();

    let (refresh_result, output) = refresh
        .await
        .unwrap_or_else(|error| panic!("refresh task should complete normally: {error}"));
    stop_server.notify_one();
    let requests = provider_server
        .await
        .unwrap_or_else(|error| panic!("loopback provider task should complete: {error}"));

    must_ok(refresh_result);
    assert_eq!(
        requests.len(),
        2,
        "only the in-flight Responses usage and reset-credit reads should complete; stale credentials must not start the later route-band reads"
    );
    assert_eq!(
        *lock_test_mutex(&observer.intents, "weekly floor intents"),
        Vec::<WeeklyQuotaFloorIntent>::new(),
        "a generation-rejected response must not signal a floor intent"
    );
    assert_eq!(
        std::str::from_utf8(&output).expect("refresh output should be UTF-8"),
        "refreshed: 0\n"
    );

    let observation = state
        .load_account_credit_observation(&account_id)
        .await
        .expect("replacement-generation observation should read")
        .expect("generation replacement should preserve its credit marker");
    assert_eq!(observation.credential_generation(), 2);
    assert_eq!(
        observation.provider_observation().availability(),
        &codex_router_core::credit_usage::CreditAvailability::Unknown,
        "generation-one provider facts must not overwrite generation-two authority"
    );
}

#[path = "quota_initial_resolution_tests.rs"]
mod quota_initial_resolution_tests;
