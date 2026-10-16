use super::*;

#[derive(Clone, Copy)]
enum ConfiguredPool {
    Empty,
    ClaudeOnly,
    DisabledOpenai,
    ExhaustedOpenai,
}

fn expected_pool_error(pool: ConfiguredPool) -> serde_json::Value {
    match pool {
        ConfiguredPool::Empty | ConfiguredPool::ClaudeOnly => serde_json::json!({
            "type": "error", "status": 503,
            "error": {
                "type": "configuration_error",
                "code": "codex_router_no_openai_accounts",
                "message": "No OpenAI accounts are configured for this Router. Add an OpenAI account to this Router and sign in."
            }
        }),
        ConfiguredPool::DisabledOpenai | ConfiguredPool::ExhaustedOpenai => serde_json::json!({
            "type": "error", "status": 429,
            "error": {
                "type": "usage_limit_reached",
                "code": "codex_router_all_accounts_exhausted",
                "message": "All configured codex-router accounts are out of usable quota."
            }
        }),
    }
}

fn check_pool_error(pool: ConfiguredPool, websocket: bool) {
    let directory = ProxyTestTempDir::new("no_openai_accounts_error");
    let database = directory.path().join("state.sqlite");
    let secret_root = directory.path().join("secrets");
    let setup = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    setup.block_on(async {
        must_ok(AsyncSqliteStateStore::open(&database).await);
    });
    drop(setup);
    let state = must_ok(SqliteStateStore::open(&database));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    match pool {
        ConfiguredPool::Empty => {}
        ConfiguredPool::ClaudeOnly => {
            must_ok(
                state.upsert_account(
                    &AccountRecord::new(
                        Provider::Claude,
                        account_id("claude_pool_account"),
                        "claude-pool-canary",
                        AccountStatus::Enabled,
                    )
                    .with_active_credential_generation(1),
                ),
            );
        }
        ConfiguredPool::ExhaustedOpenai => {
            let account = AccountRecord::new(
                Provider::Openai,
                account_id("pool_error_account"),
                "pool-error-canary",
                AccountStatus::Enabled,
            );
            persist_account_with_snapshot_and_token(
                &state,
                &secrets,
                &account,
                0,
                "pool-error-token-canary",
            );
        }
        ConfiguredPool::DisabledOpenai => {
            must_ok(state.upsert_account(&AccountRecord::new(
                Provider::Openai,
                account_id("disabled_pool_account"),
                "disabled-pool-canary",
                AccountStatus::Disabled,
            )));
        }
    }
    let upstream = must_ok(TcpListener::bind("127.0.0.1:0"));
    must_ok(upstream.set_nonblocking(true));
    let endpoint = must_ok(UpstreamEndpoint::new(format!(
        "http://{}/v1",
        must_ok(upstream.local_addr()),
    )));
    let runtime = must_ok(LoopbackRouterRuntime::start_for_test(
        LoopbackRouterRuntimeConfig::new(
            must_ok(LoopbackBindAddress::new("127.0.0.1", 0)),
            endpoint,
            database,
            secret_root,
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        )
        .with_quota_clock(1_030, 60),
    ));
    let address = runtime.local_addr();
    let client = thread::spawn(move || {
        if websocket {
            let mut request = must_ok(format!("ws://{address}/v1/responses").into_client_request());
            request.headers_mut().insert(
                "Authorization",
                HeaderValue::from_static("Bearer current-token"),
            );
            let (mut client, _) = must_ok(connect_with_config(
                request,
                Some(unbounded_test_websocket_config()),
                0,
            ));
            must_ok(client.send(Message::text(
                r#"{"type":"response.create","model":"gpt-6-luna","input":[]}"#,
            )));
            let body = must_ok(client.read()).to_string();
            let _ = client.close(None);
            body
        } else {
            let response = send_loopback_request(
                address,
                "POST /v1/responses HTTP/1.1\r\n",
                br#"{"model":"gpt-6-luna","input":[]}"#,
            );
            assert!(response.starts_with("HTTP/1.1 503"), "{response}");
            response
                .split_once("\r\n\r\n")
                .expect("HTTP body separator")
                .1
                .to_owned()
        }
    });
    must_ok(runtime.serve_protocol_connections(1));
    let body = client.join().expect("pool-error client should settle");
    let observed: serde_json::Value = must_ok(serde_json::from_str(&body));
    assert_eq!(observed, expected_pool_error(pool));
    assert!(!body.contains("canary"));
    assert!(
        matches!(upstream.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[test]
fn http_zero_openai_accounts_reports_configuration_instead_of_quota() {
    check_pool_error(ConfiguredPool::Empty, false);
}

#[test]
fn websocket_zero_openai_accounts_reports_configuration_instead_of_quota() {
    check_pool_error(ConfiguredPool::Empty, true);
}

#[test]
fn http_claude_only_pool_reports_missing_openai_configuration() {
    check_pool_error(ConfiguredPool::ClaudeOnly, false);
}

#[test]
fn websocket_claude_only_pool_reports_missing_openai_configuration() {
    check_pool_error(ConfiguredPool::ClaudeOnly, true);
}

#[test]
fn configured_disabled_or_exhausted_openai_accounts_keep_existing_error() {
    for pool in [
        ConfiguredPool::DisabledOpenai,
        ConfiguredPool::ExhaustedOpenai,
    ] {
        check_pool_error(pool, false);
        check_pool_error(pool, true);
    }
}

#[tokio::test]
async fn excluded_openai_account_is_still_configured() {
    let directory = ProxyTestTempDir::new("excluded_openai_is_configured");
    let state = must_ok(AsyncSqliteStateStore::open(&directory.path().join("state.sqlite")).await);
    let configured_id = account_id("configured_excluded_account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Openai,
                    configured_id.clone(),
                    "configured-excluded-canary",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let selector = AsyncRepositoryBackedAccountSelector::new(&state);
    let request =
        HttpProxyRequest::new(Method::Post, "/v1/responses").with_excluded_account(configured_id);

    let error = selector
        .select_upstream_account(&request, TokenGeneration::new(1), None)
        .await
        .expect_err("excluded account should remain unavailable");

    assert!(matches!(
        error,
        HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        }
    ));
}
