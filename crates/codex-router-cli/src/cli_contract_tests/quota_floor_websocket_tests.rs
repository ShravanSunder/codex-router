use super::quota_snapshot_tests::FaultingFloorRefreshProvider;
use super::*;
use sqlx::Connection;

#[test]
#[allow(clippy::result_large_err)]
fn saved_floor_refresh_reconnects_established_websocket_before_later_response_create() {
    let test_root = TestRoot::new("joined-floor-websocket");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let floor_account_id = account_id("acct_a_floor_socket");
    let healthy_account_id = account_id("acct_b_healthy_socket");
    let excluded_cli_account_id = account_id("acct_c_floor_excluded_cli");
    let seed_account =
        |account_id: &AccountId, label: &str, weekly_remaining: u32, access_token: &str| {
            must_ok(AccountStateRepository::upsert_account(
                &state,
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    label,
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            ));
            let key = must_ok(account_credential_bundle_key(account_id, 1));
            must_ok(
                secrets.write_secret(
                    &key,
                    &must_ok(
                        AccountCredentialBundle::imported_codex_auth(
                            access_token,
                            Some("fixture-refresh-canary".to_owned()),
                        )
                        .with_expires_unix_seconds(2_000)
                        .to_secret_string(),
                    ),
                ),
            );
            let windows = [
                PersistedSelectorQuotaWindow::new(
                    account_id.clone(),
                    "responses",
                    18_000,
                    SelectorQuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(100)
                .with_reset_unix_seconds(18_000)
                .with_effective(true)
                .with_observed_unix_seconds(1_000),
                PersistedSelectorQuotaWindow::new(
                    account_id.clone(),
                    "responses",
                    604_800,
                    SelectorQuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(weekly_remaining)
                .with_reset_unix_seconds(604_800)
                .with_observed_unix_seconds(1_000),
            ];
            must_ok(
                SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                    &state,
                    account_id,
                    "responses",
                    &windows,
                    1_000,
                    2_000,
                ),
            );
            must_ok(QuotaSnapshotRepository::upsert_snapshot(
                &state,
                &PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                    .with_observed_unix_seconds(1_000)
                    .with_route_band("responses", 100)
                    .with_reset_unix_seconds(18_000),
            ));
        };
    seed_account(
        &floor_account_id,
        "floor-socket",
        9,
        "floor-socket-access-canary",
    );
    migrate_test_state_database(&state_path);

    let initial_status = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(test_root.path()),
            "--format",
            "json",
            "--no-refresh",
            "--now-unix-seconds",
            "1100",
        ],
        CliContext::new(Vec::new()),
    );
    let initial_json: serde_json::Value = must_ok(serde_json::from_str(&initial_status.stdout));
    assert!(
        initial_json["accounts"]
            .as_array()
            .is_some_and(|accounts| accounts
                .iter()
                .any(|account| account["safe_account_label"] == "floor-socket"
                    && account["preferred_next"] == true,)),
        "initial selector: {:?}",
        initial_json["accounts"].as_array().map(|accounts| accounts
            .iter()
            .map(|account| (
                account["safe_account_label"].as_str(),
                account["availability"].as_str(),
                account["routing_reason"].as_str(),
                account["preferred_next"].as_bool(),
            ))
            .collect::<Vec<_>>())
    );

    let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let upstream_address = must_ok(upstream_listener.local_addr());
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let (completion_ack_sender, completion_ack_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        for connection_index in 0..2 {
            let (stream, _) = must_ok(upstream_listener.accept());
            must_ok(stream.set_read_timeout(Some(Duration::from_secs(2))));
            let mut websocket = must_ok(accept_hdr(
                stream,
                |request: &Request, response: Response| {
                    let authorization = request
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("<missing>")
                        .to_owned();
                    must_ok(upstream_sender.send(authorization));
                    Ok(response)
                },
            ));
            let first_frame = must_ok(websocket.read());
            if connection_index == 0 {
                assert_eq!(
                    first_frame.to_string(),
                    r#"{"type":"response.create","turn":1}"#
                );
                must_ok(websocket.send(Message::text(r#"{"type":"response.output_text.delta"}"#)));
                must_ok(websocket.send(Message::text(r#"{"type":"response.completed","turn":1}"#)));
                let after_failed_refresh = must_ok(websocket.read());
                assert_eq!(
                    after_failed_refresh.to_string(),
                    r#"{"type":"response.create","turn":2}"#
                );
                must_ok(websocket.send(Message::text(r#"{"type":"response.completed","turn":2}"#)));
                let after_successful_floor = websocket.read();
                assert!(
                    !matches!(after_successful_floor, Ok(ref frame) if frame.to_string().contains("response.create"))
                );
            } else {
                assert_eq!(
                    first_frame.to_string(),
                    r#"{"type":"response.create","turn":4}"#
                );
                must_ok(websocket.send(Message::text(r#"{"type":"response.completed"}"#)));
                must_ok(completion_ack_receiver.recv_timeout(Duration::from_secs(2)));
            }
        }
    });
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        must_ok(LoopbackBindAddress::new("127.0.0.1", 0)),
        must_ok(UpstreamEndpoint::new(format!(
            "http://{upstream_address}/v1"
        ))),
        state_path.clone(),
        secret_root.clone(),
    )
    .with_quota_clock(1_201, 300);
    let router = must_ok(LoopbackRouterRuntime::start(config));
    let router_port = router.local_addr().port();
    let floor_notifier = router.websocket_quota_floor_notifier();
    let router_thread = thread::spawn(move || router.serve_protocol_connections(2));

    let mut first_client = connect_tokenless_websocket_with_retry(router_port);
    match first_client.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => {
            must_ok(stream.set_read_timeout(Some(Duration::from_secs(2))));
        }
        _ => panic!("local floor WebSocket should use a plain TCP stream"),
    }
    must_ok(first_client.send(Message::text(r#"{"type":"response.create","turn":1}"#)));
    assert_eq!(
        first_client
            .read()
            .unwrap_or_else(|error| panic!(
                "first floor socket initial response read failed: {error}"
            ))
            .to_string(),
        r#"{"type":"response.output_text.delta"}"#
    );
    assert_eq!(
        first_client
            .read()
            .unwrap_or_else(|error| panic!("first floor socket completion read failed: {error}"))
            .to_string(),
        r#"{"type":"response.completed","turn":1}"#
    );
    assert_eq!(
        must_ok(upstream_receiver.recv_timeout(Duration::from_secs(2))),
        "Bearer floor-socket-access-canary"
    );

    seed_account(
        &healthy_account_id,
        "healthy-socket",
        80,
        "healthy-socket-access-canary",
    );
    seed_account(
        &excluded_cli_account_id,
        "floor-excluded-cli",
        9,
        "floor-excluded-cli-access-canary",
    );
    let mutation = must_ok(
        test_async_runtime().block_on(AsyncWeeklyQuotaFloorMutationStore::open(&state_path)),
    );
    must_ok(
        test_async_runtime().block_on(mutation.set_weekly_quota_floor_by_account_id(
            &floor_account_id,
            Some(must_ok(WeeklyQuotaFloorBasisPoints::new(1_000))),
        )),
    );
    must_ok(
        test_async_runtime().block_on(mutation.set_weekly_quota_floor_by_account_id(
            &excluded_cli_account_id,
            Some(must_ok(WeeklyQuotaFloorBasisPoints::new(1_000))),
        )),
    );
    test_async_runtime().block_on(mutation.close());
    let edited_status = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(test_root.path()),
            "--format",
            "json",
            "--no-refresh",
            "--now-unix-seconds",
            "1100",
        ],
        CliContext::new(Vec::new()),
    );
    let edited_json: serde_json::Value = must_ok(serde_json::from_str(&edited_status.stdout));
    assert!(
        edited_json["accounts"]
            .as_array()
            .is_some_and(
                |accounts| accounts.iter().any(|account| account["safe_account_label"]
                    == "floor-excluded-cli"
                    && account["routing_exclusion"] == "excluded_weekly_quota_floor"
                    && account["preferred_next"] == false,)
            ),
        "edited selector: {:?}",
        edited_json["accounts"].as_array().map(|accounts| accounts
            .iter()
            .map(|account| (
                account["safe_account_label"].as_str(),
                account["routing_exclusion"].as_str(),
                account["preferred_next"].as_bool(),
            ))
            .collect::<Vec<_>>())
    );

    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_100);
    let failed_provider = FaultingFloorRefreshProvider {
        database_path: state_path.clone(),
        failure_stage: "history",
    };
    let mut failed_output = Vec::new();
    let failed = must_err(refresh_quota_store_paths_with_floor_observer(
        &mut failed_output,
        &state_path,
        &secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &failed_provider,
        QuotaRefreshObservationContext {
            observed_unix_seconds: 1_200,
            weekly_floor_observer: Some(&floor_notifier),
        },
    ));
    assert!(failed.to_string().contains("sqlite state store failed"));
    let retained_after_failed_refresh = must_ok(
        SelectorQuotaRepository::selector_inputs_for_route_band(&state, "responses", 1_200),
    );
    assert!(retained_after_failed_refresh.iter().any(|account| {
        account.account_id() == &floor_account_id
            && account.windows().iter().any(|window| {
                window.limit_window_seconds() == 604_800 && window.remaining_headroom() == 9
            })
    }));
    must_ok(first_client.send(Message::text(r#"{"type":"response.create","turn":2}"#)));
    assert_eq!(
        first_client
            .read()
            .unwrap_or_else(|error| panic!(
                "first floor socket post-failure completion read failed: {error}"
            ))
            .to_string(),
        r#"{"type":"response.completed","turn":2}"#
    );
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&state_path)
        .create_if_missing(false);
    let mut fault_connection =
        must_ok(test_async_runtime().block_on(sqlx::SqliteConnection::connect_with(&options)));
    must_ok(test_async_runtime().block_on(
        sqlx::query("DROP TRIGGER injected_history_failure").execute(&mut fault_connection),
    ));
    must_ok(test_async_runtime().block_on(fault_connection.close()));

    let provider = JoinedFloorCrossingProvider {
        floor_account_id: floor_account_id.clone(),
    };
    let mut output = Vec::new();
    must_ok(refresh_quota_store_paths_with_floor_observer(
        &mut output,
        &state_path,
        &secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        QuotaRefreshObservationContext {
            observed_unix_seconds: 1_200,
            weekly_floor_observer: Some(&floor_notifier),
        },
    ));
    let saved = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
        &state,
        "responses",
        1_200,
    ));
    assert!(saved.iter().any(|account| {
        account.account_id() == &floor_account_id
            && account.windows().iter().any(|window| {
                window.limit_window_seconds() == 604_800 && window.remaining_headroom() == 8
            })
    }));
    let reconnect = first_client
        .read()
        .unwrap_or_else(|error| panic!("first floor socket reconnect read failed: {error}"))
        .to_string();
    assert!(reconnect.contains("websocket_connection_limit_reached"));
    let _old_socket_create =
        first_client.send(Message::text(r#"{"type":"response.create","turn":3}"#));
    drop(first_client);

    let mut second_client = connect_tokenless_websocket_with_retry(router_port);
    must_ok(second_client.send(Message::text(r#"{"type":"response.create","turn":4}"#)));
    assert_eq!(
        second_client
            .read()
            .unwrap_or_else(|error| panic!("fallback floor socket completion read failed: {error}"))
            .to_string(),
        r#"{"type":"response.completed"}"#
    );
    must_ok(completion_ack_sender.send(()));
    assert_eq!(
        must_ok(upstream_receiver.recv_timeout(Duration::from_secs(2))),
        "Bearer healthy-socket-access-canary"
    );
    drop(second_client);
    must_ok(must_ok(
        router_thread.join().map_err(|_| "router thread failed"),
    ));
    must_ok(upstream_thread.join().map_err(|_| "upstream thread failed"));
}
