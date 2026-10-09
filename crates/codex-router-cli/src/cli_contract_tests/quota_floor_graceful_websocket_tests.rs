use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_switch_band_refresh_waits_for_active_turn_then_reconnects_to_healthy_peer() {
    run_saved_switch_band_websocket_case(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_switch_band_refresh_keeps_active_socket_when_no_healthy_peer_exists() {
    run_saved_switch_band_websocket_case(false).await;
}

#[allow(clippy::result_large_err)]
async fn run_saved_switch_band_websocket_case(with_healthy_peer: bool) {
    let test_root = TestRoot::new("joined-graceful-floor-websocket");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let floor_account_id = account_id("acct_a_graceful_floor");
    let healthy_account_id = account_id("acct_b_graceful_peer");
    let seed_account = |account_id: &AccountId, label: &str, remaining: u32, access: &str| {
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
        let key = must_ok(openai_account_credential_bundle_key(account_id, 1));
        let bundle = AccountCredentialBundle::imported_codex_auth(
            access,
            Some("fixture-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000);
        must_ok(secrets.write_secret(&key, &must_ok(bundle.to_secret_string())));
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
            .with_remaining_headroom(remaining)
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
    };
    seed_account(
        &floor_account_id,
        "graceful-floor",
        9,
        "graceful-floor-access",
    );
    let mutation = must_ok(AsyncWeeklyQuotaFloorMutationStore::open(&state_path).await);
    must_ok(
        mutation
            .set_weekly_quota_floor_by_account_id(
                &floor_account_id,
                Some(must_ok(WeeklyQuotaFloorBasisPoints::new(500))),
            )
            .await,
    );
    mutation.close().await;

    let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let upstream_address = must_ok(upstream_listener.local_addr());
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let (completion_ack_sender, completion_ack_receiver) = mpsc::channel();
    let (finish_turn_sender, finish_turn_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        for connection_index in 0..if with_healthy_peer { 2 } else { 1 } {
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
                must_ok(finish_turn_receiver.recv_timeout(Duration::from_secs(2)));
                must_ok(websocket.send(Message::text(r#"{"type":"response.completed"}"#)));
                let after_completion = websocket.read();
                if with_healthy_peer {
                    assert!(
                        !matches!(after_completion, Ok(ref frame) if frame.to_string().contains("response.create")),
                        "pending next turn must not reach the old account"
                    );
                } else {
                    assert_eq!(
                        must_ok(after_completion).to_string(),
                        r#"{"type":"response.create","turn":2}"#
                    );
                    must_ok(websocket.send(Message::text(r#"{"type":"response.completed"}"#)));
                    must_ok(completion_ack_receiver.recv_timeout(Duration::from_secs(2)));
                }
            } else {
                assert_eq!(
                    first_frame.to_string(),
                    r#"{"type":"response.create","turn":2}"#
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
    .with_quota_clock(1_100, 300);
    let router = must_ok(
        agent_proxy_services::test_support::activate_core_fixture(config, secrets.clone()).await,
    );
    let router_port = router.local_addr().port();
    let floor_notifier = router.websocket_quota_floor_notifier();
    let router_task = tokio::spawn(async move {
        router
            .serve_protocol_connections(if with_healthy_peer { 2 } else { 1 })
            .await
            .unwrap_or_else(|error| {
                panic!("router runtime should serve websocket connections: {error}")
            });
    });

    let mut first_client = connect_tokenless_websocket_with_retry(router_port);
    must_ok(first_client.send(Message::text(r#"{"type":"response.create","turn":1}"#)));
    assert_eq!(
        must_ok(first_client.read()).to_string(),
        r#"{"type":"response.output_text.delta"}"#
    );
    assert_eq!(
        must_ok(upstream_receiver.recv_timeout(Duration::from_secs(2))),
        "Bearer graceful-floor-access"
    );
    if with_healthy_peer {
        seed_account(
            &healthy_account_id,
            "graceful-peer",
            80,
            "graceful-peer-access",
        );
    }
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_100);
    let provider = JoinedFloorCrossingProvider { floor_account_id };
    let mut output = Vec::new();
    let _report = must_ok(
        refresh_quota_store_paths_with_dependencies_and_floor_notifier_async(
            &mut output,
            &state_path,
            &secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            QuotaRefreshObservationContext {
                observed_unix_seconds: 1_200,
                schedule: crate::quota::QuotaRefreshSchedule::Manual,
                weekly_floor_observer: Some(&floor_notifier),
            },
        )
        .await,
    );
    must_ok(first_client.send(Message::text(r#"{"type":"response.create","turn":2}"#)));
    must_ok(finish_turn_sender.send(()));
    assert_eq!(
        must_ok(first_client.read()).to_string(),
        r#"{"type":"response.completed"}"#
    );
    if with_healthy_peer {
        assert!(
            must_ok(first_client.read())
                .to_string()
                .contains("websocket_connection_limit_reached")
        );
        drop(first_client);

        let mut second_client = connect_tokenless_websocket_with_retry(router_port);
        must_ok(second_client.send(Message::text(r#"{"type":"response.create","turn":2}"#)));
        assert_eq!(
            must_ok(second_client.read()).to_string(),
            r#"{"type":"response.completed"}"#
        );
        must_ok(completion_ack_sender.send(()));
        assert_eq!(
            must_ok(upstream_receiver.recv_timeout(Duration::from_secs(2))),
            "Bearer graceful-peer-access"
        );
        drop(second_client);
    } else {
        assert_eq!(
            must_ok(first_client.read()).to_string(),
            r#"{"type":"response.completed"}"#
        );
        must_ok(completion_ack_sender.send(()));
        drop(first_client);
    }
    must_ok(router_task.await.map_err(|_| "router task failed"));
    must_ok(upstream_thread.join().map_err(|_| "upstream thread failed"));
}
