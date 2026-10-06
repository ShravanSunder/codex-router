#![allow(clippy::panic)]

#[cfg(not(feature = "keychain-test-support"))]
compile_error!("proxy overload process tests require keychain-test-support");

#[path = "support/proxy_overload_fixture.rs"]
mod proxy_overload_fixture;

use futures_util::SinkExt;
use proxy_overload_fixture::RouterFixture;
use proxy_overload_fixture::UpstreamSession;
use proxy_overload_fixture::connect_local_client;
use proxy_overload_fixture::receive_client_frame;
use proxy_overload_fixture::send_explicit_response_create;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const VALID_THREAD_ID: &str = "thread_proxy_overload_valid";
const REPEATED_OVERLOAD_COUNT: usize = 3;
const OVERLOAD_REQUEST_COUNT: usize = 4 * REPEATED_OVERLOAD_COUNT;
const ROUTER_SESSION_COUNT: usize = 5;
const NO_AUTOMATIC_REQUEST_WINDOW: Duration = Duration::from_millis(40);

struct HeaderControlRunContext<'a> {
    router: &'a mut RouterFixture,
    observations: &'a mut Vec<String>,
    explicit_client_request_count: &'a mut usize,
    upstream_request_counts: &'a mut Vec<usize>,
}

struct OverloadFrameCase {
    name: &'static str,
    provider_payload: &'static str,
    expected_client_payload: &'static str,
}

const OVERLOAD_FRAME_CASES: [OverloadFrameCase; 4] = [
    OverloadFrameCase {
        name: "server_is_overloaded response.failed envelope",
        provider_payload: r#"{"type":"response.failed","response":{"id":"resp_overload_owner","status":"failed","error":{"code":"server_is_overloaded","message":"fixture overload"}}}"#,
        expected_client_payload: r#"{"type":"response.failed","response":{"id":"resp_overload_owner","status":"failed","error":{"code":"server_is_overloaded","message":"fixture overload"}}}"#,
    },
    OverloadFrameCase {
        name: "server_is_overloaded error envelope",
        provider_payload: r#"{"type":"error","status":503,"error":{"code":"server_is_overloaded","message":"fixture overload"}}"#,
        expected_client_payload: r#"{"type":"error","status":503,"error":{"code":"server_is_overloaded","message":"fixture overload"}}"#,
    },
    OverloadFrameCase {
        name: "slow_down error envelope",
        provider_payload: r#"{"type":"error","status":503,"error":{"code":"slow_down","message":"fixture slowdown"}}"#,
        expected_client_payload: r#"{"type":"error","status":503,"error":{"code":"slow_down","message":"fixture slowdown"}}"#,
    },
    OverloadFrameCase {
        name: "slow_down response.failed envelope",
        provider_payload: r#"{"type":"response.failed","response":{"id":"resp_overload_owner","status":"failed","error":{"code":"slow_down","message":"fixture slowdown"}}}"#,
        expected_client_payload: r#"{"type":"response.failed","response":{"id":"resp_overload_owner","status":"failed","error":{"code":"slow_down","message":"fixture slowdown"}}}"#,
    },
];

#[tokio::test]
async fn compiled_router_forwards_overload_and_keeps_explicit_requests_on_same_socket() {
    let mut router = RouterFixture::start(ROUTER_SESSION_COUNT)
        .await
        .unwrap_or_else(|error| panic!("isolated Router fixture should start: {error}"));
    let mut observations = Vec::new();
    let mut upstream_request_counts = Vec::with_capacity(ROUTER_SESSION_COUNT);
    let mut explicit_client_request_count = 1;
    let valid_thread_id_values = vec![VALID_THREAD_ID.to_owned()];
    let mut valid_client = connect_local_client(router.router_port(), &valid_thread_id_values)
        .await
        .unwrap_or_else(|error| {
            panic!("explicit valid-header WebSocket client should connect: {error}")
        });

    let first_request_payload = send_explicit_response_create(&mut valid_client, 1)
        .await
        .unwrap_or_else(|error| panic!("first explicit response.create should send: {error}"));
    let mut valid_upstream = router
        .accept_upstream()
        .await
        .unwrap_or_else(|error| panic!("valid-header Router upstream should connect: {error}"));
    let selected_account_id = validate_handshake_observation(
        "valid thread-id session",
        &valid_upstream,
        &valid_thread_id_values,
        router.account_ids(),
    )
    .unwrap_or_else(|error| {
        panic!("valid thread-id upstream handshake should be genuine: {error}")
    });
    let initial_request_observations = observations.len();
    record_explicit_upstream_request(
        &mut observations,
        "valid thread-id session request 1",
        &mut valid_upstream,
        &first_request_payload,
    )
    .await;
    assert_eq!(
        observations.len(),
        initial_request_observations,
        "the first explicit client request must reach the actual Router upstream unchanged"
    );

    let mut valid_connection_stayed_open = true;
    let mut next_request_sequence = 2;
    let mut first_request_is_already_sent = true;
    'overload_matrix: for _repeat in 0..REPEATED_OVERLOAD_COUNT {
        for frame_case in &OVERLOAD_FRAME_CASES {
            if first_request_is_already_sent {
                first_request_is_already_sent = false;
            } else {
                let request_payload = send_explicit_response_create(
                    &mut valid_client,
                    next_request_sequence,
                )
                .await
                .unwrap_or_else(|error| {
                    observations.push(format!(
                        "{}: explicit client request {} failed after the preceding overload: {error}",
                        frame_case.name, next_request_sequence
                    ));
                    String::new()
                });
                if request_payload.is_empty() {
                    valid_connection_stayed_open = false;
                    break 'overload_matrix;
                }
                explicit_client_request_count += 1;
                let observations_before_request = observations.len();
                record_explicit_upstream_request(
                    &mut observations,
                    frame_case.name,
                    &mut valid_upstream,
                    &request_payload,
                )
                .await;
                if observations.len() != observations_before_request {
                    valid_connection_stayed_open = false;
                    break 'overload_matrix;
                }
                next_request_sequence += 1;
            }

            if let Err(error) = valid_upstream
                .send_provider_frame(frame_case.provider_payload)
                .await
            {
                observations.push(format!(
                    "{}: provider fixture send failed: {error}",
                    frame_case.name
                ));
                valid_connection_stayed_open = false;
                break 'overload_matrix;
            }
            match receive_client_frame(&mut valid_client).await {
                Ok(Message::Text(actual_payload)) => {
                    let actual_payload = actual_payload.to_string();
                    if actual_payload != frame_case.expected_client_payload {
                        observations.push(format!(
                            "{}: expected original provider payload {:?}, received {:?}",
                            frame_case.name, frame_case.expected_client_payload, actual_payload
                        ));
                        valid_connection_stayed_open = false;
                        break 'overload_matrix;
                    }
                }
                Ok(other_message) => {
                    observations.push(format!(
                        "{}: expected original provider text frame, received {other_message:?}",
                        frame_case.name
                    ));
                    valid_connection_stayed_open = false;
                    break 'overload_matrix;
                }
                Err(error) => {
                    observations.push(format!(
                        "{}: client frame read failed: {error}",
                        frame_case.name
                    ));
                    valid_connection_stayed_open = false;
                    break 'overload_matrix;
                }
            }

            let observations_before_probe = observations.len();
            record_no_automatic_upstream_frame(
                &mut observations,
                frame_case.name,
                &mut valid_upstream,
            )
            .await;
            if observations.len() != observations_before_probe {
                valid_connection_stayed_open = false;
                break 'overload_matrix;
            }
        }
    }

    if OVERLOAD_REQUEST_COUNT <= 10
        || OVERLOAD_FRAME_CASES.len() * REPEATED_OVERLOAD_COUNT != OVERLOAD_REQUEST_COUNT
    {
        observations
            .push("overload test matrix must exceed ten distinct explicit requests".to_owned());
    }
    if valid_connection_stayed_open && next_request_sequence != OVERLOAD_REQUEST_COUNT + 1 {
        observations.push(format!(
            "valid same-socket matrix sent {} overload requests, expected {OVERLOAD_REQUEST_COUNT}",
            next_request_sequence.saturating_sub(1)
        ));
        valid_connection_stayed_open = false;
    }
    if valid_connection_stayed_open && next_request_sequence == OVERLOAD_REQUEST_COUNT + 1 {
        let statusless_provider_payload = r#"{"type":"error","error":{"code":"server_is_overloaded","message":"statusless fixture overload"}}"#;
        let statusless_expected_client_payload = r#"{"type":"error","error":{"code":"server_is_overloaded","message":"statusless fixture overload"}}"#;
        let statusless_request =
            send_explicit_response_create(&mut valid_client, next_request_sequence)
                .await
                .unwrap_or_else(|error| {
                    panic!("statusless-case response.create should send: {error}")
                });
        explicit_client_request_count += 1;
        record_explicit_upstream_request(
            &mut observations,
            "statusless overload request",
            &mut valid_upstream,
            &statusless_request,
        )
        .await;
        valid_upstream
            .send_provider_frame(statusless_provider_payload)
            .await
            .unwrap_or_else(|error| panic!("statusless overload should reach Router: {error}"));
        assert_client_received_literal(
            &mut valid_client,
            statusless_expected_client_payload,
            &mut observations,
            "statusless overload",
        )
        .await;
        let statusless_terminal = r#"{"type":"response.completed","response":{"id":"resp_statusless_terminal","status":"completed"}}"#;
        valid_upstream
            .send_provider_frame(statusless_terminal)
            .await
            .unwrap_or_else(|error| {
                panic!("real terminal should release the statusless turn: {error}")
            });
        assert_client_received_literal(
            &mut valid_client,
            statusless_terminal,
            &mut observations,
            "statusless-case real terminal",
        )
        .await;

        let post_terminal_request =
            send_explicit_response_create(&mut valid_client, next_request_sequence + 1)
                .await
                .unwrap_or_else(|error| {
                    panic!("next explicit request should send after the real terminal: {error}")
                });
        explicit_client_request_count += 1;
        record_explicit_upstream_request(
            &mut observations,
            "explicit request after statusless-case terminal",
            &mut valid_upstream,
            &post_terminal_request,
        )
        .await;
        let ordinary_reply = r#"{"type":"response.completed","response":{"id":"resp_after_overload","status":"completed"}}"#;
        valid_upstream
            .send_provider_frame(ordinary_reply)
            .await
            .unwrap_or_else(|error| panic!("ordinary continuation response should send: {error}"));
        assert_client_received_literal(
            &mut valid_client,
            ordinary_reply,
            &mut observations,
            "ordinary continuation response",
        )
        .await;
        record_no_automatic_upstream_frame(
            &mut observations,
            "ordinary continuation response",
            &mut valid_upstream,
        )
        .await;
        close_client_connection(&mut valid_client, &mut valid_upstream, &mut observations).await;
        upstream_request_counts.push(valid_upstream.explicit_response_create_count());
    } else {
        upstream_request_counts.push(valid_upstream.explicit_response_create_count());
        drop(valid_client);
        drop(valid_upstream);
    }

    let header_controls = [
        ("no thread-id", Vec::new(), Vec::new()),
        ("empty thread-id", vec![String::new()], vec![String::new()]),
        (
            "duplicate thread-id",
            vec![
                "thread_duplicate_a".to_owned(),
                "thread_duplicate_b".to_owned(),
            ],
            vec!["thread_duplicate_b".to_owned()],
        ),
        (
            "overlong thread-id",
            vec!["x".repeat(257)],
            vec!["x".repeat(257)],
        ),
    ];
    for (control_index, (control_name, thread_id_values, forwarded_thread_id_values)) in
        header_controls.iter().enumerate()
    {
        run_header_control(
            HeaderControlRunContext {
                router: &mut router,
                observations: &mut observations,
                explicit_client_request_count: &mut explicit_client_request_count,
                upstream_request_counts: &mut upstream_request_counts,
            },
            control_name,
            thread_id_values,
            forwarded_thread_id_values,
            control_index + 100,
        )
        .await;
    }

    let unexpected_upstream_connection = router
        .has_unexpected_upstream_connection(Duration::from_millis(100))
        .await
        .unwrap_or_else(|error| panic!("check for extra upstream handshakes: {error}"));
    let accepted_upstream_connection_count = router.upstream_connection_count();
    router.close_upstream_listener();

    let registry_report = router.wait_for_exit().await.unwrap_or_else(|error| {
        panic!("Router should finish its exact client connection set: {error}")
    });
    assert_eq!(
        accepted_upstream_connection_count, ROUTER_SESSION_COUNT,
        "one upstream handshake is expected per explicit client connection"
    );
    assert_eq!(
        upstream_request_counts.iter().sum::<usize>(),
        explicit_client_request_count,
        "the fake upstream must receive each explicit client request exactly once"
    );
    assert_eq!(
        registry_report
            .get("handled_connections")
            .and_then(|value| value.as_u64()),
        Some(ROUTER_SESSION_COUNT as u64),
        "Router must reap its exact local client connection set before red/green assertions"
    );
    assert!(
        !unexpected_upstream_connection,
        "Router opened an extra upstream WebSocket without a client connection"
    );
    assert!(
        observations.is_empty(),
        "actual Router overload forwarding observations: {observations:#?}"
    );
    assert_eq!(
        upstream_request_counts,
        [OVERLOAD_REQUEST_COUNT + 2, 1, 1, 1, 1],
        "the same valid-header socket carries 12 overloads, statusless and continuation requests"
    );
    assert_eq!(
        explicit_client_request_count,
        OVERLOAD_REQUEST_COUNT + 2 + (ROUTER_SESSION_COUNT - 1),
        "the explicit client request total matches the frame/header matrix"
    );

    let web_socket_registry = registry_report
        .get("websocket_registry")
        .unwrap_or_else(|| panic!("Router report should contain websocket_registry"));
    assert_eq!(
        web_socket_registry
            .get("quota_reconnect_signal_count")
            .and_then(|value| value.as_u64()),
        Some(0),
        "overload must not emit a quota reconnect signal"
    );

    let persisted_state = router
        .inspect_persisted_state(&selected_account_id)
        .await
        .unwrap_or_else(|error| panic!("read fixture account state after Router exit: {error}"));
    assert_eq!(
        persisted_state.quota_exhaustion_state_rows, 0,
        "overload must not persist account quota exhaustion"
    );
    assert_eq!(
        persisted_state.quota_exhaustion_snapshot_rows, 0,
        "overload must not write a quota-exhaustion snapshot"
    );
    assert_eq!(
        persisted_state.eligible_selector_window_rows, 4,
        "both accounts must retain their two eligible quota windows"
    );
    assert!(
        persisted_state.response_owner_rows_for_selected_account > 0,
        "same-account response.id owner bookkeeping remains enabled"
    );
}

async fn run_header_control(
    context: HeaderControlRunContext<'_>,
    control_name: &str,
    thread_id_values: &[String],
    forwarded_thread_id_values: &[String],
    request_sequence: usize,
) {
    let HeaderControlRunContext {
        router,
        observations,
        explicit_client_request_count,
        upstream_request_counts,
    } = context;
    let mut client = connect_local_client(router.router_port(), thread_id_values)
        .await
        .unwrap_or_else(|error| panic!("{control_name} WebSocket client should connect: {error}"));
    let client_request = send_explicit_response_create(&mut client, request_sequence)
        .await
        .unwrap_or_else(|error| panic!("{control_name} response.create should send: {error}"));
    *explicit_client_request_count += 1;
    let mut upstream = router
        .accept_upstream()
        .await
        .unwrap_or_else(|error| panic!("{control_name} Router upstream should connect: {error}"));
    validate_handshake_observation(
        control_name,
        &upstream,
        forwarded_thread_id_values,
        router.account_ids(),
    )
    .unwrap_or_else(|error| panic!("{control_name} upstream handshake should be genuine: {error}"));
    record_explicit_upstream_request(observations, control_name, &mut upstream, &client_request)
        .await;

    let provider_payload = r#"{"type":"error","status":503,"error":{"code":"slow_down","message":"header-control fixture"}}"#;
    let expected_client_payload = r#"{"type":"error","status":503,"error":{"code":"slow_down","message":"header-control fixture"}}"#;
    upstream
        .send_provider_frame(provider_payload)
        .await
        .unwrap_or_else(|error| panic!("{control_name} provider frame should send: {error}"));
    assert_client_received_literal(
        &mut client,
        expected_client_payload,
        observations,
        control_name,
    )
    .await;
    record_no_automatic_upstream_frame(observations, control_name, &mut upstream).await;

    let completion = r#"{"type":"response.completed","response":{"id":"resp_header_control","status":"completed"}}"#;
    upstream
        .send_provider_frame(completion)
        .await
        .unwrap_or_else(|error| panic!("{control_name} terminal should send: {error}"));
    assert_client_received_literal(&mut client, completion, observations, control_name).await;
    close_client_connection(&mut client, &mut upstream, observations).await;
    upstream_request_counts.push(upstream.explicit_response_create_count());
}

fn validate_handshake_observation(
    session_name: &str,
    upstream: &UpstreamSession,
    expected_thread_id_values: &[String],
    fixture_account_ids: &[String; 2],
) -> Result<String, String> {
    if upstream.handshake.request_path != "/v1/responses" {
        return Err(format!(
            "expected /v1/responses upstream path, got {:?}",
            upstream.handshake.request_path
        ));
    }
    if upstream.handshake.thread_id_values != expected_thread_id_values {
        return Err(format!(
            "{session_name}: expected forwarded thread-id values {expected_thread_id_values:?}, got {:?}",
            upstream.handshake.thread_id_values
        ));
    }
    let selected_account_id = upstream
        .handshake
        .selected_account_id
        .as_ref()
        .filter(|value| {
            fixture_account_ids
                .iter()
                .any(|fixture_id| fixture_id == *value)
        })
        .cloned()
        .ok_or_else(|| {
            format!(
                "Router did not select one of the seeded accounts for {session_name}: {:?}",
                upstream.handshake.selected_account_id
            )
        })?;
    Ok(selected_account_id)
}

async fn record_explicit_upstream_request(
    observations: &mut Vec<String>,
    session_name: &str,
    upstream: &mut UpstreamSession,
    expected_request: &str,
) {
    match upstream.receive_client_frame().await {
        Ok(Message::Text(actual_request)) if actual_request.as_str() == expected_request => {}
        Ok(Message::Text(actual_request)) => observations.push(format!(
            "{session_name}: upstream expected explicit client request {expected_request:?}, got {:?}",
            actual_request.as_str()
        )),
        Ok(other_frame) => observations.push(format!(
            "{session_name}: upstream expected explicit client text request, got {other_frame:?}"
        )),
        Err(error) => observations.push(format!(
            "{session_name}: upstream did not receive explicit client request: {error}"
        )),
    }
}

async fn assert_client_received_literal(
    client: &mut proxy_overload_fixture::TestLocalWebSocket,
    expected_payload: &str,
    observations: &mut Vec<String>,
    frame_name: &str,
) {
    match receive_client_frame(client).await {
        Ok(Message::Text(actual_payload)) if actual_payload.as_str() == expected_payload => {}
        Ok(Message::Text(actual_payload)) => observations.push(format!(
            "{frame_name}: expected original provider payload {expected_payload:?}, received {:?}",
            actual_payload.as_str()
        )),
        Ok(other_frame) => observations.push(format!(
            "{frame_name}: expected provider text frame, received {other_frame:?}"
        )),
        Err(error) => observations.push(format!("{frame_name}: client frame read failed: {error}")),
    }
}

async fn record_no_automatic_upstream_frame(
    observations: &mut Vec<String>,
    frame_name: &str,
    upstream: &mut UpstreamSession,
) {
    match upstream
        .receive_unrequested_frame(NO_AUTOMATIC_REQUEST_WINDOW)
        .await
    {
        Ok(None) => {}
        Ok(Some(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
        Ok(Some(Message::Text(text))) => observations.push(format!(
            "{frame_name}: Router sent an unsolicited upstream text frame: {:?}",
            text.as_str()
        )),
        Ok(Some(Message::Binary(_))) => observations.push(format!(
            "{frame_name}: Router sent an unsolicited upstream binary frame"
        )),
        Ok(Some(Message::Close(_))) => observations.push(format!(
            "{frame_name}: Router closed the same upstream socket after overload"
        )),
        Err(error) => observations.push(format!(
            "{frame_name}: same-socket no-replay probe failed: {error}"
        )),
    }
}

async fn close_client_connection(
    client: &mut proxy_overload_fixture::TestLocalWebSocket,
    upstream: &mut UpstreamSession,
    observations: &mut Vec<String>,
) {
    if let Err(error) = client.send(Message::Close(None)).await {
        observations.push(format!("explicit client Close should send: {error}"));
        return;
    }
    match upstream.receive_client_frame().await {
        Ok(Message::Close(_)) => {}
        Ok(other_frame) => observations.push(format!(
            "fake upstream expected explicit client Close, received {other_frame:?}"
        )),
        Err(error) => observations.push(format!(
            "fake upstream did not receive client Close: {error}"
        )),
    }
}
