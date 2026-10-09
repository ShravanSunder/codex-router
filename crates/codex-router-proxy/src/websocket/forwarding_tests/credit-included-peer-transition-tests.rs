use super::*;

use super::credit_turn_test_support::CREDIT_TURN_FIXTURE_TIME;
use super::credit_turn_test_support::CreditTurnFixture;
use super::credit_turn_test_support::CreditTurnTestDirectory;
use crate::server::LoopbackBindAddress;
use crate::server::LoopbackRouterRuntime;
use crate::server::LoopbackRouterRuntimeConfig;
use crate::upstream::UpstreamEndpoint;
use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditBalance;
use codex_router_core::credit_usage::CreditProviderLimitReason;
use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_core::credit_usage::CreditSpendControl;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::openai_account_credential_bundle_key;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::sqlite::SqliteStateStore;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::WebSocket;
use tokio_tungstenite::tungstenite::accept_hdr;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::connect;
use tokio_tungstenite::tungstenite::handshake::server::Request;
use tokio_tungstenite::tungstenite::handshake::server::Response;
use tokio_tungstenite::tungstenite::stream::MaybeTlsStream;

type LoopbackWebSocket = WebSocket<MaybeTlsStream<TcpStream>>;

const CREDIT_SOURCE_TOKEN: &str = "credit-turn-source-token";
const INCLUDED_PEER_TOKEN: &str = "credit-included-peer-token";
const OLD_SOCKET_RECONNECT_TURN: u64 = 2;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assembled_websocket_credit_source_yields_after_terminal_to_included_peer() {
    let directory = CreditTurnTestDirectory::new();
    let fixture = CreditTurnFixture::new(&directory).await;
    let secret_path = fixture
        .database_path
        .parent()
        .unwrap_or_else(|| panic!("credit-turn state should have a parent directory"))
        .join("secrets");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .unwrap_or_else(|error| panic!("credit-turn credentials should open: {error}"));

    let credit_observation = CreditProviderObservation::new(
        CreditAvailability::Available {
            balance: Some(CreditBalance::new("2.75").expect("credit balance is valid")),
        },
        CreditSpendControl::Clear,
        Some(CreditProviderLimitReason::RateLimitReached),
    );
    fixture
        .replace_responses_snapshot(
            exhausted_credit_windows(&fixture.account_id),
            &credit_observation,
        )
        .await;
    persist_websocket_credential(&secrets, &fixture.account_id, CREDIT_SOURCE_TOKEN);

    let peer_id = AccountId::new("acct_credit_turn_included_peer")
        .expect("included peer account id should validate");
    let (upstream_listener, upstream_address) = bind_loopback_upstream();
    let (terminal_release_sender, terminal_release_receiver) = mpsc::channel();
    let (credit_authorization_sender, credit_authorization_receiver) = mpsc::channel();
    let (first_credit_create_sender, first_credit_create_receiver) = mpsc::channel();
    let (old_upstream_after_terminal_sender, old_upstream_after_terminal_receiver) =
        mpsc::channel();
    let (peer_authorization_sender, peer_authorization_receiver) = mpsc::channel();
    let (peer_create_sender, peer_create_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (credit_stream, _peer) = upstream_listener.accept().unwrap_or_else(|error| {
            panic!("credit upstream should accept the old socket: {error}")
        });
        let mut credit_upstream = accept_upstream_websocket(
            credit_stream,
            &credit_authorization_sender,
            "credit upstream",
        );
        let session_update = read_upstream_text(&mut credit_upstream, "credit session update");
        assert_eq!(session_update, r#"{"type":"session.update"}"#);
        credit_upstream
            .send(Message::text(r#"{"type":"session.ready"}"#))
            .unwrap_or_else(|error| panic!("credit upstream should ready the old socket: {error}"));

        let first_create = read_upstream_text(&mut credit_upstream, "initial credit create");
        first_credit_create_sender
            .send(first_create)
            .unwrap_or_else(|error| panic!("initial create observation should send: {error}"));
        credit_upstream
            .send(Message::text(
                r#"{"type":"response.output_text.delta","turn":1}"#,
            ))
            .unwrap_or_else(|error| panic!("credit output should send: {error}"));
        terminal_release_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!("active credit turn terminal should release: {error}"));
        credit_upstream
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .unwrap_or_else(|error| panic!("active credit terminal should send: {error}"));

        let old_followup = match credit_upstream.read() {
            Ok(Message::Text(frame)) => {
                let frame = frame.to_string();
                if frame.contains("response.create") {
                    credit_upstream
                        .send(Message::text(
                            r#"{"type":"response.output_text.delta","turn":2}"#,
                        ))
                        .unwrap_or_else(|error| {
                            panic!("unexpected old turn two output should send: {error}")
                        });
                    credit_upstream
                        .send(Message::text(r#"{"type":"response.completed","turn":2}"#))
                        .unwrap_or_else(|error| {
                            panic!("unexpected old turn two terminal should send: {error}")
                        });
                }
                frame
            }
            Ok(Message::Close(_)) => "<close>".to_owned(),
            Ok(other) => format!("<non-text:{other}>"),
            Err(_error) => "<closed>".to_owned(),
        };
        old_upstream_after_terminal_sender
            .send(old_followup)
            .unwrap_or_else(|error| {
                panic!("old upstream terminal boundary should record: {error}")
            });

        let (peer_stream, _peer) = upstream_listener.accept().unwrap_or_else(|error| {
            panic!("included peer upstream should accept the new socket: {error}")
        });
        let mut peer_upstream = accept_upstream_websocket(
            peer_stream,
            &peer_authorization_sender,
            "included peer upstream",
        );
        let peer_session_update = read_upstream_text(&mut peer_upstream, "peer session update");
        assert_eq!(peer_session_update, r#"{"type":"session.update"}"#);
        peer_upstream
            .send(Message::text(r#"{"type":"session.ready"}"#))
            .unwrap_or_else(|error| panic!("peer upstream should ready the new socket: {error}"));
        let peer_create = read_upstream_text(&mut peer_upstream, "included peer create");
        peer_create_sender
            .send(peer_create)
            .unwrap_or_else(|error| panic!("peer create observation should send: {error}"));
        peer_upstream
            .send(Message::text(
                r#"{"type":"response.output_text.delta","turn":1,"source":"included-peer"}"#,
            ))
            .unwrap_or_else(|error| panic!("included peer output should send: {error}"));
        peer_upstream
            .send(Message::text(
                r#"{"type":"response.completed","turn":1,"source":"included-peer"}"#,
            ))
            .unwrap_or_else(|error| panic!("included peer terminal should send: {error}"));
    });

    let bind_address = LoopbackBindAddress::new("127.0.0.1", 0)
        .unwrap_or_else(|error| panic!("router bind address should validate: {error}"));
    let endpoint = UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
        .unwrap_or_else(|error| panic!("WebSocket upstream endpoint should validate: {error}"));
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        bind_address,
        endpoint,
        fixture.database_path.clone(),
        secret_path,
    )
    .with_quota_clock(CREDIT_TURN_FIXTURE_TIME, 300);
    let runtime_secrets = secrets.clone();
    let (router_address_sender, router_address_receiver) = mpsc::channel();
    let runtime_thread = tokio::spawn(async move {
        let runtime =
            LoopbackRouterRuntime::start_with_credentials_for_test(config, runtime_secrets)
                .await
                .unwrap_or_else(|error| {
                    panic!("assembled WebSocket runtime should start: {error}")
                });
        router_address_sender
            .send(runtime.local_addr())
            .unwrap_or_else(|error| panic!("router address should reach the test: {error}"));
        runtime.serve_protocol_connections(2).await
    });
    let router_address = router_address_receiver
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("assembled WebSocket runtime should bind: {error}"));

    let (active_output_sender, active_output_receiver) = mpsc::channel();
    let (send_second_create_sender, send_second_create_receiver) = mpsc::channel();
    let (second_create_sent_sender, second_create_sent_receiver) = mpsc::channel();
    let client_thread = thread::spawn(move || {
        let mut old_client = connect_local_websocket(router_address, "old credit client");
        old_client
            .send(Message::text(r#"{"type":"session.update"}"#))
            .unwrap_or_else(|error| panic!("old client session update should send: {error}"));
        let old_ready = read_local_text(&mut old_client, "old client session.ready");
        assert_eq!(old_ready, r#"{"type":"session.ready"}"#);
        old_client
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .unwrap_or_else(|error| panic!("initial credit turn should send: {error}"));
        let active_output = read_local_text(&mut old_client, "active credit output");
        active_output_sender
            .send(active_output.clone())
            .unwrap_or_else(|error| panic!("active output observation should send: {error}"));
        send_second_create_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| {
                panic!("second create should release after peer commit: {error}")
            });
        old_client
            .send(Message::text(format!(
                r#"{{"type":"response.create","turn":{OLD_SOCKET_RECONNECT_TURN}}}"#
            )))
            .unwrap_or_else(|error| panic!("second credit create should send: {error}"));
        second_create_sent_sender
            .send(())
            .unwrap_or_else(|error| panic!("second create send boundary should signal: {error}"));

        let mut old_frames = vec![active_output];
        loop {
            match old_client.read() {
                Ok(message) => {
                    let frame = message.to_string();
                    let reconnect_received = frame == CODEX_WEBSOCKET_RECONNECT_SIGNAL;
                    old_frames.push(frame);
                    if reconnect_received {
                        break;
                    }
                }
                Err(error) => {
                    old_frames.push(format!("<client-read-error:{error}>"));
                    break;
                }
            }
        }
        let _old_close = old_client.close(None);

        let mut peer_client = connect_local_websocket(router_address, "included peer client");
        peer_client
            .send(Message::text(r#"{"type":"session.update"}"#))
            .unwrap_or_else(|error| panic!("peer client session update should send: {error}"));
        let peer_ready = read_local_text(&mut peer_client, "peer client session.ready");
        assert_eq!(peer_ready, r#"{"type":"session.ready"}"#);
        peer_client
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .unwrap_or_else(|error| panic!("included peer create should send: {error}"));
        let peer_output = read_local_text(&mut peer_client, "included peer output");
        let peer_terminal = read_local_text(&mut peer_client, "included peer terminal");
        let _peer_close = peer_client.close(None);
        (old_frames, peer_output, peer_terminal)
    });

    assert_eq!(
        credit_authorization_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!(
                "old socket should use the selected credit token: {error}"
            )),
        format!("Bearer {CREDIT_SOURCE_TOKEN}")
    );
    assert_eq!(
        first_credit_create_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!("initial create should reach credit upstream: {error}")),
        r#"{"type":"response.create","turn":1}"#
    );
    assert_eq!(
        active_output_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!(
                "active credit output should arrive before peer commit: {error}"
            )),
        r#"{"type":"response.output_text.delta","turn":1}"#
    );

    persist_included_peer(
        &fixture.database_path,
        &secrets,
        &peer_id,
        INCLUDED_PEER_TOKEN,
    );
    send_second_create_sender
        .send(())
        .unwrap_or_else(|error| panic!("active client should send the next create: {error}"));
    second_create_sent_receiver
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|error| {
            panic!("next create should reach the client write boundary: {error}")
        });
    terminal_release_sender.send(()).unwrap_or_else(|error| {
        panic!("active terminal should release after queued create: {error}")
    });

    let (old_socket_frames, peer_output, peer_terminal) = client_thread
        .join()
        .unwrap_or_else(|error| panic!("old and new WebSocket clients should finish: {error:?}"));
    let old_upstream_after_terminal = old_upstream_after_terminal_receiver
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("old upstream turn boundary should be observed: {error}"));
    assert_eq!(
        peer_authorization_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!(
                "new connection should resolve the included peer token: {error}"
            )),
        format!("Bearer {INCLUDED_PEER_TOKEN}")
    );
    assert_eq!(
        peer_create_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!(
                "new peer create should reach its selected upstream: {error}"
            )),
        r#"{"type":"response.create","turn":1}"#
    );
    let handled_connections = runtime_thread
        .await
        .unwrap_or_else(|error| panic!("assembled WebSocket runtime should not panic: {error:?}"))
        .unwrap_or_else(|error| {
            panic!("assembled WebSocket runtime should serve two sockets: {error}")
        });
    upstream_thread
        .join()
        .unwrap_or_else(|error| panic!("loopback WebSocket upstream should finish: {error:?}"));

    assert_eq!(handled_connections, 2);
    assert_eq!(
        old_socket_frames,
        [
            r#"{"type":"response.output_text.delta","turn":1}"#,
            r#"{"type":"response.completed","turn":1}"#,
            CODEX_WEBSOCKET_RECONNECT_SIGNAL,
        ],
        "the old turn output and terminal must arrive before its reconnect signal"
    );
    assert_ne!(
        old_upstream_after_terminal,
        format!(r#"{{"type":"response.create","turn":{OLD_SOCKET_RECONNECT_TURN}}}"#),
        "the denied next create must not reach the credit-backed upstream"
    );
    assert_eq!(
        peer_output,
        r#"{"type":"response.output_text.delta","turn":1,"source":"included-peer"}"#
    );
    assert_eq!(
        peer_terminal,
        r#"{"type":"response.completed","turn":1,"source":"included-peer"}"#
    );

    fixture
        .writer
        .close()
        .await
        .expect("credit-turn writer should close");
    fixture
        .reader
        .close()
        .await
        .expect("credit-turn reader should close");
}

fn bind_loopback_upstream() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("WebSocket upstream should bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("WebSocket upstream address should read: {error}"));
    (listener, address)
}

// Tungstenite's handshake callback fixes the large HTTP response error type.
#[allow(clippy::result_large_err)]
fn accept_upstream_websocket(
    stream: TcpStream,
    authorization_sender: &mpsc::Sender<String>,
    label: &str,
) -> tokio_tungstenite::tungstenite::WebSocket<TcpStream> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|error| panic!("{label} read timeout should set: {error}"));
    accept_hdr(stream, |request: &Request, response: Response| {
        let authorization = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("<missing>")
            .to_owned();
        authorization_sender
            .send(authorization)
            .unwrap_or_else(|error| panic!("{label} authorization should record: {error}"));
        Ok(response)
    })
    .unwrap_or_else(|error| panic!("{label} WebSocket should upgrade: {error}"))
}

fn read_upstream_text(
    websocket: &mut tokio_tungstenite::tungstenite::WebSocket<TcpStream>,
    label: &str,
) -> String {
    websocket
        .read()
        .unwrap_or_else(|error| panic!("{label} frame should arrive: {error}"))
        .to_string()
}

fn connect_local_websocket(address: SocketAddr, label: &str) -> LoopbackWebSocket {
    let request = format!("ws://{address}/v1/responses")
        .into_client_request()
        .unwrap_or_else(|error| panic!("{label} request should build: {error}"));
    let (mut client, _response) =
        connect(request).unwrap_or_else(|error| panic!("{label} should connect: {error}"));
    match client.get_mut() {
        MaybeTlsStream::Plain(stream) => stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap_or_else(|error| panic!("{label} read timeout should set: {error}")),
        _ => panic!("{label} should use a loopback TCP stream"),
    }
    client
}

fn read_local_text(client: &mut LoopbackWebSocket, label: &str) -> String {
    client
        .read()
        .unwrap_or_else(|error| panic!("{label} frame should arrive: {error}"))
        .to_string()
}

fn exhausted_credit_windows(account_id: &AccountId) -> Vec<PersistedSelectorQuotaWindow> {
    [18_000_u64, 604_800]
        .into_iter()
        .enumerate()
        .map(|(index, window_seconds)| {
            PersistedSelectorQuotaWindow::new(
                account_id.clone(),
                RouteBand::Responses.as_str(),
                window_seconds,
                SelectorQuotaWindowStatus::Ineligible,
            )
            .with_remaining_headroom(0)
            .with_reset_unix_seconds(CREDIT_TURN_FIXTURE_TIME + window_seconds)
            .with_effective(index == 0)
            .with_observed_unix_seconds(CREDIT_TURN_FIXTURE_TIME)
        })
        .collect()
}

fn persist_included_peer(
    database_path: &std::path::Path,
    secrets: &EncryptedCredentialStore,
    peer_id: &AccountId,
    access_token: &str,
) {
    let state = SqliteStateStore::open(database_path)
        .unwrap_or_else(|error| panic!("included peer state should open: {error}"));
    let peer = AccountRecord::new(
        Provider::Openai,
        peer_id.clone(),
        "credit-included-peer",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    AccountStateRepository::upsert_account(&state, &peer)
        .unwrap_or_else(|error| panic!("included peer account should persist: {error}"));
    let windows = [18_000_u64, 604_800]
        .into_iter()
        .enumerate()
        .map(|(index, window_seconds)| {
            PersistedSelectorQuotaWindow::new(
                peer_id.clone(),
                RouteBand::Responses.as_str(),
                window_seconds,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(90)
            .with_reset_unix_seconds(CREDIT_TURN_FIXTURE_TIME + window_seconds)
            .with_effective(index == 0)
            .with_observed_unix_seconds(CREDIT_TURN_FIXTURE_TIME)
        })
        .collect::<Vec<_>>();
    SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        &state,
        peer_id,
        RouteBand::Responses.as_str(),
        &windows,
        CREDIT_TURN_FIXTURE_TIME,
        CREDIT_TURN_FIXTURE_TIME + 300,
    )
    .unwrap_or_else(|error| panic!("included peer windows should persist: {error}"));
    let snapshot =
        PersistedQuotaSnapshot::new(peer_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
            .with_observed_unix_seconds(CREDIT_TURN_FIXTURE_TIME)
            .with_route_band(RouteBand::Responses.as_str(), 90)
            .with_reset_unix_seconds(CREDIT_TURN_FIXTURE_TIME + 18_000)
            .with_stale_penalty(false);
    QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot)
        .unwrap_or_else(|error| panic!("included peer snapshot should persist: {error}"));
    let key = openai_account_credential_bundle_key(peer_id, 1)
        .unwrap_or_else(|error| panic!("included peer credential key should build: {error}"));
    let bundle = AccountCredentialBundle::imported_codex_auth(
        access_token,
        Some(format!("{access_token}-refresh")),
    )
    .with_expires_unix_seconds(CREDIT_TURN_FIXTURE_TIME + 1_000)
    .to_secret_string()
    .unwrap_or_else(|error| panic!("included peer credential should serialize: {error}"));
    secrets
        .write_secret(&key, &bundle)
        .unwrap_or_else(|error| panic!("included peer credential should persist: {error}"));
}

fn persist_websocket_credential(
    secrets: &EncryptedCredentialStore,
    account_id: &AccountId,
    access_token: &str,
) {
    let key = openai_account_credential_bundle_key(account_id, 1)
        .unwrap_or_else(|error| panic!("credit source credential key should build: {error}"));
    let bundle = AccountCredentialBundle::imported_codex_auth(
        access_token,
        Some(format!("{access_token}-refresh")),
    )
    .with_expires_unix_seconds(CREDIT_TURN_FIXTURE_TIME + 1_000)
    .to_secret_string()
    .unwrap_or_else(|error| panic!("credit source credential should serialize: {error}"));
    secrets
        .write_secret(&key, &bundle)
        .unwrap_or_else(|error| panic!("credit source credential should persist: {error}"));
}
