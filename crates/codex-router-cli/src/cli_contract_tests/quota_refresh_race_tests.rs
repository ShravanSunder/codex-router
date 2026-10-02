use super::*;

struct CredentialGenerationRaceResolver {
    stale_account_id: AccountId,
    later_account_id: AccountId,
    stale_resolution_started: tokio::sync::Notify,
    allow_stale_resolution: tokio::sync::Notify,
}

impl AsyncProviderCredentialResolver for CredentialGenerationRaceResolver {
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        if account_id == &self.stale_account_id {
            self.stale_resolution_started.notify_one();
            self.allow_stale_resolution.notified().await;
            return Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                SecretString::new("stale-generation-token"),
                2,
            ));
        }

        if account_id == &self.later_account_id {
            return Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                SecretString::new("later-account-token"),
                1,
            ));
        }

        Err(CredentialResolverError::AccountUnavailable)
    }
}

#[tokio::test]
async fn generation_race_skips_only_stale_account_and_refreshes_later_accounts() {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    let test_root = TestRoot::new("quota-refresh-generation-race");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&state_path).await);
    let stale_account_id = account_id("acct_a_generation_race");
    let later_account_id = account_id("acct_z_later_refresh");
    let stale_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        stale_account_id.clone(),
        "stale-generation",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    let later_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        later_account_id.clone(),
        "later-account",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(state.upsert_account(&stale_account).await);
    must_ok(state.upsert_account(&later_account).await);

    let listener = must_ok(tokio::net::TcpListener::bind("127.0.0.1:0").await);
    let address = must_ok(listener.local_addr());
    let stop_server = Arc::new(tokio::sync::Notify::new());
    let server_stop = Arc::clone(&stop_server);
    let provider_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        loop {
            tokio::select! {
                incoming = listener.accept() => {
                    let (mut stream, _peer_address) = match incoming {
                        Ok(connection) => connection,
                        Err(error) => panic!("loopback quota provider should accept: {error}"),
                    };
                    let mut request_bytes = Vec::new();
                    let mut byte = [0_u8; 1];
                    loop {
                        let bytes_read = match stream.read(&mut byte).await {
                            Ok(bytes_read) => bytes_read,
                            Err(error) => panic!("loopback quota provider should read: {error}"),
                        };
                        if bytes_read == 0 {
                            break;
                        }
                        request_bytes.push(byte[0]);
                        if request_bytes.ends_with(b"\r\n\r\n") {
                            break;
                        }
                    }
                    let request = String::from_utf8_lossy(&request_bytes).into_owned();
                    let is_usage_request = request.starts_with("GET /api/codex/usage HTTP/1.1\r\n");
                    let is_reset_request = request.starts_with(
                        "GET /api/codex/rate-limit-reset-credits HTTP/1.1\r\n",
                    );
                    assert!(
                        is_usage_request || is_reset_request,
                        "unexpected loopback quota request: {request:?}"
                    );
                    assert!(
                        request.to_ascii_lowercase().contains(
                            "authorization: bearer later-account-token\r\n"
                        ),
                        "only the later account may reach the provider: {request:?}"
                    );
                    requests.push(request);

                    let body = if is_usage_request {
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
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    if let Err(error) = stream.write_all(response.as_bytes()).await {
                        panic!("loopback quota provider should respond: {error}");
                    }
                }
                () = server_stop.notified() => return requests,
            }
        }
    });

    let resolver = Arc::new(CredentialGenerationRaceResolver {
        stale_account_id: stale_account_id.clone(),
        later_account_id: later_account_id.clone(),
        stale_resolution_started: tokio::sync::Notify::new(),
        allow_stale_resolution: tokio::sync::Notify::new(),
    });
    let secret_root = test_root.path().join("unused-secrets");
    let provider = must_ok(HttpQuotaRefreshProvider::new());
    let refresh_state_path = state_path.clone();
    let refresh_resolver = Arc::clone(&resolver);
    let mut refresh = tokio::spawn(async move {
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
    let stale_resolution_started = resolver.stale_resolution_started.notified();
    tokio::pin!(stale_resolution_started);
    tokio::select! {
        () = &mut stale_resolution_started => {}
        result = &mut refresh => panic!(
            "refresh should pause inside the first account's resolver before completing: {result:?}"
        ),
    }

    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    stale_account_id.clone(),
                    "stale-generation",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(3),
            )
            .await,
    );
    resolver.allow_stale_resolution.notify_one();
    let (refresh_result, output) = match refresh.await {
        Ok(result) => result,
        Err(error) => panic!("refresh task should complete normally: {error}"),
    };
    stop_server.notify_one();
    let requests = match provider_server.await {
        Ok(requests) => requests,
        Err(error) => panic!("loopback quota provider task should finish: {error}"),
    };

    must_ok(refresh_result);
    assert_eq!(
        requests.len(),
        4,
        "later account refresh makes two usage and two reset reads"
    );
    assert_eq!(
        std::str::from_utf8(&output).expect("refresh output should be UTF-8"),
        "refresh skipped: account=stale-generation error=credential generation changed during refresh\nrefreshed: 2\nfailed: 2\n"
    );

    let later_observation = state
        .load_account_credit_observation(&later_account_id)
        .await
        .expect("later account observation should read")
        .expect("later account Responses read should commit");
    assert_eq!(later_observation.latest_started_attempt(), 1);
    assert_eq!(later_observation.committed_attempt(), Some(1));
    let stale_observation = state
        .load_account_credit_observation(&stale_account_id)
        .await
        .expect("stale account observation should read");
    assert!(stale_observation.is_none_or(|observation| {
        observation.credential_generation() != 3
            && observation.provider_observation().availability()
                == &codex_router_core::credit_usage::CreditAvailability::Unknown
    }));
}
