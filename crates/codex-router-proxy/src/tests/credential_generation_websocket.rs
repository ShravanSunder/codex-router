use super::*;

const FIXED_QUOTA_TIME: u64 = 1_030;
const ROUTE_BAND: &str = "responses";
const SHORT_WINDOW_SECONDS: u64 = 18_000;
const WEEKLY_WINDOW_SECONDS: u64 = 604_800;

#[test]
#[allow(clippy::result_large_err)]
fn assembled_loopback_websocket_preserves_ordinary_socket_across_credential_generation_renewal() {
    let temp_dir = ProxyTestTempDir::new("ordinary_websocket_credential_renewal");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let state = SqliteStateStore::open(&database_path)
        .unwrap_or_else(|error| panic!("ordinary socket state should open: {error}"));
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .unwrap_or_else(|error| panic!("ordinary socket secrets should open: {error}"));
    let account_id = account_id("acct_ordinary_credential_renewal");
    let account = AccountRecord::new(
        Provider::Openai,
        account_id.clone(),
        "ordinary",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    AccountStateRepository::upsert_account(&state, &account)
        .unwrap_or_else(|error| panic!("ordinary socket account should persist: {error}"));
    save_generation_credential(&secrets, &account_id, 1, "generation-one-token");
    save_included_quota_snapshot(&state, &account_id, FIXED_QUOTA_TIME);
    persist_disallow_credit_policy(&database_path, &account_id);

    let initial_input = SelectorQuotaRepository::selector_inputs_for_route_band(
        &state,
        ROUTE_BAND,
        FIXED_QUOTA_TIME,
    )
    .unwrap_or_else(|error| panic!("initial ordinary selector input should load: {error}"))
    .into_iter()
    .find(|input| input.account_id() == &account_id)
    .unwrap_or_else(|| panic!("initial ordinary account should be in selector input"));
    assert_eq!(initial_input.active_credential_generation(), Some(1));
    assert_eq!(
        initial_input.credit_usage_policy(),
        codex_router_core::credit_usage::CreditUsagePolicy::Disallow
    );

    let upstream_listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("ordinary mock upstream should bind: {error}"));
    let upstream_address = upstream_listener
        .local_addr()
        .unwrap_or_else(|error| panic!("ordinary upstream address should read: {error}"));
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (stream, _) = upstream_listener
            .accept()
            .unwrap_or_else(|error| panic!("ordinary upstream should accept one socket: {error}"));
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap_or_else(|error| panic!("ordinary upstream read timeout should set: {error}"));
        let mut websocket = accept_hdr(stream, |request: &Request, response: Response| {
            let authorization = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>")
                .to_owned();
            upstream_sender
                .send(("authorization".to_owned(), authorization))
                .unwrap_or_else(|error| panic!("upstream authorization should record: {error}"));
            Ok(response)
        })
        .unwrap_or_else(|error| panic!("ordinary upstream WebSocket should upgrade: {error}"));

        let session_update = websocket.read().unwrap_or_else(|error| {
            panic!("ordinary upstream should receive session update: {error}")
        });
        upstream_sender
            .send(("first_frame".to_owned(), session_update.to_string()))
            .unwrap_or_else(|error| panic!("upstream session update should record: {error}"));
        websocket
            .send(Message::text(r#"{"type":"session.ready"}"#))
            .unwrap_or_else(|error| panic!("ordinary upstream should ready the session: {error}"));

        for turn in 1..=3 {
            let next_frame = websocket.read().map(|message| message.to_string());
            let expected_create = format!(r#"{{"type":"response.create","turn":{turn}}}"#);
            upstream_sender
                .send((
                    format!("turn_{turn}"),
                    next_frame
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|error| format!("upstream read failed: {error}")),
                ))
                .unwrap_or_else(|error| panic!("upstream next frame should record: {error}"));
            if !matches!(next_frame.as_ref(), Ok(frame) if frame == &expected_create) {
                break;
            }
            websocket
                .send(Message::text(format!(
                    r#"{{"type":"response.output_text.delta","turn":{turn}}}"#
                )))
                .unwrap_or_else(|error| panic!("ordinary upstream output should send: {error}"));
            websocket
                .send(Message::text(format!(
                    r#"{{"type":"response.completed","turn":{turn}}}"#
                )))
                .unwrap_or_else(|error| {
                    panic!("ordinary upstream completion should send: {error}")
                });
        }
    });

    let endpoint = UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
        .unwrap_or_else(|error| panic!("ordinary upstream endpoint should validate: {error}"));
    let bind_address = LoopbackBindAddress::new("127.0.0.1", 0)
        .unwrap_or_else(|error| panic!("ordinary router bind address should validate: {error}"));
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        bind_address,
        endpoint,
        database_path.clone(),
        secret_path,
    )
    .with_quota_clock(FIXED_QUOTA_TIME, 300);
    let runtime = LoopbackRouterRuntime::start(config, secrets.clone().into())
        .unwrap_or_else(|error| panic!("assembled ordinary runtime should start: {error}"));
    let router_address = runtime.local_addr();
    let runtime_thread = thread::spawn(move || runtime.serve_protocol_connections(1));
    let (client_ready_sender, client_ready_receiver) = mpsc::channel();
    let (continue_sender, continue_receiver) = mpsc::channel();
    let (client_turn_sender, client_turn_receiver) = mpsc::channel();
    let client_thread = thread::spawn(move || {
        let request = format!("ws://{router_address}/v1/responses")
            .into_client_request()
            .unwrap_or_else(|error| {
                panic!("ordinary local WebSocket request should build: {error}")
            });
        let (mut client, _) = connect(request)
            .unwrap_or_else(|error| panic!("ordinary local WebSocket should connect: {error}"));
        match client.get_mut() {
            tokio_tungstenite::tungstenite::stream::MaybeTlsStream::Plain(stream) => stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap_or_else(|error| panic!("ordinary client read timeout should set: {error}")),
            _ => panic!("ordinary local WebSocket should use loopback TCP"),
        }
        client
            .send(Message::text(r#"{"type":"session.update"}"#))
            .unwrap_or_else(|error| panic!("ordinary client session update should send: {error}"));
        let ready = client
            .read()
            .unwrap_or_else(|error| panic!("ordinary client should receive session ready: {error}"))
            .to_string();
        client_ready_sender
            .send(())
            .unwrap_or_else(|error| panic!("ordinary client ready state should signal: {error}"));
        for turn in 1..=3 {
            continue_receiver
                .recv_timeout(Duration::from_secs(3))
                .unwrap_or_else(|error| {
                    panic!("ordinary client should await turn {turn}: {error}")
                });
            client
                .send(Message::text(format!(
                    r#"{{"type":"response.create","turn":{turn}}}"#
                )))
                .unwrap_or_else(|error| panic!("ordinary turn {turn} create should send: {error}"));
            let mut response_frames = Vec::new();
            let mut continue_socket = true;
            loop {
                match client.read() {
                    Ok(message) => {
                        let frame = message.to_string();
                        let is_terminal = frame.contains("response.completed")
                            || frame.contains("websocket_connection_limit_reached");
                        if frame.contains("websocket_connection_limit_reached") {
                            continue_socket = false;
                        }
                        response_frames.push(frame);
                        if is_terminal {
                            break;
                        }
                    }
                    Err(error) => {
                        response_frames.push(format!("client read failed: {error}"));
                        continue_socket = false;
                        break;
                    }
                }
            }
            client_turn_sender
                .send((turn, response_frames))
                .unwrap_or_else(|error| {
                    panic!("ordinary turn {turn} result should record: {error}")
                });
            if !continue_socket {
                break;
            }
        }
        let _close_result = client.close(None);
        ready
    });

    client_ready_receiver
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("ordinary socket should establish before renewal: {error}"));
    assert_eq!(
        upstream_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!(
                "real credential resolver should reach upstream: {error}"
            )),
        (
            "authorization".to_owned(),
            "Bearer generation-one-token".to_owned()
        )
    );
    assert_eq!(
        upstream_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!(
                "upstream should receive initial session update: {error}"
            )),
        (
            "first_frame".to_owned(),
            r#"{"type":"session.update"}"#.to_owned()
        )
    );

    continue_sender.send(()).unwrap_or_else(|error| {
        panic!("ordinary generation-one socket should start turn one: {error}")
    });
    assert_forwarded_ordinary_turn(&client_turn_receiver, &upstream_receiver, 1);

    activate_generation_two_with_invalidated_quota(&database_path, &secrets, &account_id);
    let invalidated_input = SelectorQuotaRepository::selector_inputs_for_route_band(
        &state,
        ROUTE_BAND,
        FIXED_QUOTA_TIME,
    )
    .unwrap_or_else(|error| panic!("invalidated ordinary selector input should load: {error}"))
    .into_iter()
    .find(|input| input.account_id() == &account_id)
    .unwrap_or_else(|| panic!("invalidated ordinary account should be in selector input"));
    assert_eq!(invalidated_input.active_credential_generation(), Some(2));
    assert_eq!(
        invalidated_input.credit_usage_policy(),
        codex_router_core::credit_usage::CreditUsagePolicy::Disallow
    );
    assert_eq!(invalidated_input.windows().len(), 1);
    assert_eq!(
        invalidated_input.windows()[0].limit_window_seconds(),
        SHORT_WINDOW_SECONDS
    );
    assert_eq!(
        invalidated_input.windows()[0].status(),
        SelectorQuotaWindowStatus::Ineligible
    );

    continue_sender.send(()).unwrap_or_else(|error| {
        panic!("ordinary socket should continue through invalidation: {error}")
    });
    assert_forwarded_ordinary_turn(&client_turn_receiver, &upstream_receiver, 2);

    save_included_quota_snapshot(&state, &account_id, FIXED_QUOTA_TIME);
    let refreshed_input = SelectorQuotaRepository::selector_inputs_for_route_band(
        &state,
        ROUTE_BAND,
        FIXED_QUOTA_TIME,
    )
    .unwrap_or_else(|error| panic!("refreshed ordinary selector input should load: {error}"))
    .into_iter()
    .find(|input| input.account_id() == &account_id)
    .unwrap_or_else(|| panic!("refreshed ordinary account should be in selector input"));
    assert_eq!(refreshed_input.active_credential_generation(), Some(2));
    assert_eq!(
        refreshed_input.credit_usage_policy(),
        codex_router_core::credit_usage::CreditUsagePolicy::Disallow
    );
    assert!(
        refreshed_input
            .windows()
            .iter()
            .all(|window| window.status() == SelectorQuotaWindowStatus::Eligible)
    );
    continue_sender.send(()).unwrap_or_else(|error| {
        panic!("ordinary socket should continue on refreshed included quota: {error}")
    });
    assert_forwarded_ordinary_turn(&client_turn_receiver, &upstream_receiver, 3);

    assert_eq!(
        client_thread
            .join()
            .unwrap_or_else(|error| panic!("ordinary client thread should finish: {error:?}")),
        r#"{"type":"session.ready"}"#
    );
    assert_eq!(
        runtime_thread
            .join()
            .unwrap_or_else(|error| panic!("assembled runtime thread should finish: {error:?}"))
            .unwrap_or_else(|error| panic!("assembled runtime should serve the socket: {error}")),
        1
    );
    upstream_thread
        .join()
        .unwrap_or_else(|error| panic!("ordinary upstream thread should finish: {error:?}"));
}

fn activate_generation_two_with_invalidated_quota(
    database_path: &Path,
    secrets: &EncryptedCredentialStore,
    account_id: &AccountId,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("credential activation runtime should build: {error}"));
    runtime.block_on(async {
        let async_state = AsyncSqliteStateStore::open(database_path)
            .await
            .unwrap_or_else(|error| panic!("credential activation state should open: {error}"));
        async_state
            .activate_account_credential_generation_if_current_and_invalidate_quota(
                account_id,
                1,
                2,
                AccountStatus::Enabled,
            )
            .await
            .unwrap_or_else(|error| panic!("generation two should activate: {error}"));
        async_state
            .close()
            .await
            .unwrap_or_else(|error| panic!("credential activation state should close: {error}"));
    });

    save_generation_credential(secrets, account_id, 2, "generation-two-token");
}

fn assert_forwarded_ordinary_turn(
    client_turn_receiver: &mpsc::Receiver<(u64, Vec<String>)>,
    upstream_receiver: &mpsc::Receiver<(String, String)>,
    turn: u64,
) {
    let (observed_turn, response_frames) = client_turn_receiver
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("ordinary turn {turn} response should arrive: {error}"));
    assert_eq!(observed_turn, turn);
    assert_eq!(
        response_frames,
        [
            format!(r#"{{"type":"response.output_text.delta","turn":{turn}}}"#),
            format!(r#"{{"type":"response.completed","turn":{turn}}}"#),
        ],
        "ordinary turn {turn} should deliver output and completion on the established upstream"
    );
    assert_eq!(
        upstream_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!(
                "upstream should observe ordinary turn {turn}: {error}"
            )),
        (
            format!("turn_{turn}"),
            format!(r#"{{"type":"response.create","turn":{turn}}}"#)
        )
    );
}

fn persist_disallow_credit_policy(database_path: &Path, account_id: &AccountId) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("credit policy runtime should build: {error}"));
    runtime.block_on(async {
        let async_state = AsyncSqliteStateStore::open(database_path)
            .await
            .unwrap_or_else(|error| panic!("credit policy state should open: {error}"));
        async_state
            .save_account_credit_usage_policy(
                account_id,
                codex_router_core::credit_usage::CreditUsagePolicy::Disallow,
            )
            .await
            .unwrap_or_else(|error| panic!("Disallow credit policy should persist: {error}"));
        async_state
            .close()
            .await
            .unwrap_or_else(|error| panic!("credit policy state should close: {error}"));
    });
}

fn save_generation_credential(
    secrets: &EncryptedCredentialStore,
    account_id: &AccountId,
    generation: u64,
    access_token: &str,
) {
    let key = openai_account_credential_bundle_key(account_id, generation)
        .unwrap_or_else(|error| panic!("account credential key should build: {error}"));
    let bundle = AccountCredentialBundle::imported_codex_auth(
        access_token,
        Some(format!("{access_token}-refresh")),
    )
    .with_expires_unix_seconds(FIXED_QUOTA_TIME + 1_000)
    .to_secret_string()
    .unwrap_or_else(|error| panic!("account credential bundle should serialize: {error}"));
    secrets
        .write_secret(&key, &bundle)
        .unwrap_or_else(|error| panic!("account credential should persist: {error}"));
}

fn save_included_quota_snapshot(
    state: &SqliteStateStore,
    account_id: &AccountId,
    observed_unix_seconds: u64,
) {
    let windows = [
        PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            ROUTE_BAND,
            SHORT_WINDOW_SECONDS,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(90)
        .with_effective(true)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_reset_unix_seconds(observed_unix_seconds + SHORT_WINDOW_SECONDS),
        PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            ROUTE_BAND,
            WEEKLY_WINDOW_SECONDS,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(90)
        .with_effective(false)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_reset_unix_seconds(observed_unix_seconds + WEEKLY_WINDOW_SECONDS),
    ];
    SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        state,
        account_id,
        ROUTE_BAND,
        &windows,
        observed_unix_seconds,
        observed_unix_seconds + 300,
    )
    .unwrap_or_else(|error| panic!("fresh included selector windows should persist: {error}"));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band(ROUTE_BAND, 90)
            .with_reset_unix_seconds(observed_unix_seconds + SHORT_WINDOW_SECONDS)
            .with_stale_penalty(false);
    QuotaSnapshotRepository::upsert_snapshot(state, &snapshot)
        .unwrap_or_else(|error| panic!("fresh included quota snapshot should persist: {error}"));
}
