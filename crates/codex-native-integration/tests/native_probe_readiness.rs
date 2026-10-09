use std::future::pending;
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_native_integration::AppServerProbeAction;
use codex_native_integration::CodexProtocolError;
use codex_native_integration::NativeObservationStage;
use codex_native_integration::RemoteControlObservation;
use codex_native_integration::run_app_server_probe;
use tokio::net::UnixListener;
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio::time::timeout;

#[path = "support/native_probe_fixture.rs"]
#[allow(dead_code)] // The shared fixture also supplies action-only scenarios.
mod native_probe_fixture;
use native_probe_fixture::{
    FailureCase, FixturePlan, FixtureSocket, REMOTE_WAIT, expected_initialize_request,
    expected_status_request, matches_expected_failure, serve_fixture, spawn_fixture, status,
};

#[tokio::test]
async fn wait_for_ready_retries_a_real_connect_refusal_until_the_fixture_binds() {
    let socket =
        FixtureSocket::new("wait-connect-refusal").expect("fixture directory should be created");
    let (first_refusal_sender, first_refusal_receiver) = oneshot::channel();
    let fixture_socket_path = socket.path().to_owned();
    let fixture_task = tokio::spawn(async move {
        first_refusal_receiver
            .await
            .map_err(|error| format!("first connection attempt signal failed: {error}"))?;
        let listener = UnixListener::bind(fixture_socket_path)
            .map_err(|error| format!("fixture socket should bind after refusal: {error}"))?;
        serve_fixture(
            listener,
            FixturePlan::respond_with(status("connected", "ready-host", None)),
        )
        .await
    });
    let attempts = Arc::new(AtomicUsize::new(0));
    let connector_attempts = Arc::clone(&attempts);
    let socket_path = socket.path().to_owned();
    let mut first_refusal = Some(first_refusal_sender);
    let connector = move || {
        connector_attempts.fetch_add(1, Ordering::Relaxed);
        let socket_path = socket_path.clone();
        let first_refusal = first_refusal.take();
        async move {
            let result = UnixStream::connect(socket_path).await;
            if result.is_err()
                && let Some(first_refusal) = first_refusal
            {
                let _signal_result = first_refusal.send(());
            }
            result
        }
    };

    let observation = run_app_server_probe(
        AppServerProbeAction::WaitForReady,
        Duration::from_secs(1),
        REMOTE_WAIT,
        connector,
    )
    .await
    .expect("WaitForReady should retry the refused native connect");
    let transcript = fixture_task
        .await
        .expect("fixture task should join")
        .expect("fixture exchange should complete");

    assert!(attempts.load(Ordering::Relaxed) >= 2);
    assert_eq!(
        observation.remote_control(),
        &RemoteControlObservation::Connected {
            server_name: "ready-host".to_owned(),
            environment_id: None,
        }
    );
    assert_eq!(
        transcript.initialize_request,
        Some(expected_initialize_request())
    );
    assert_eq!(transcript.action_request, Some(expected_status_request()));
}

#[tokio::test]
async fn wait_for_ready_retries_a_connect_timeout_with_the_remaining_job_budget() {
    let socket =
        FixtureSocket::new("wait-connect-timeout").expect("fixture directory should be created");
    let fixture = spawn_fixture(
        UnixListener::bind(socket.path()).expect("fixture socket should bind"),
        FixturePlan::respond_with(status("connected", "ready-host", None)),
    );
    let attempts = Arc::new(AtomicUsize::new(0));
    let connector_attempts = Arc::clone(&attempts);
    let socket_path = socket.path().to_owned();
    let connector = move || {
        let attempt_number = connector_attempts.fetch_add(1, Ordering::Relaxed);
        let socket_path = socket_path.clone();
        async move {
            if attempt_number == 0 {
                // Models a connector that stays pending to its per-attempt bound; the adjacent
                // refusal test proves retry behavior with a real Unix socket error.
                pending::<io::Result<UnixStream>>().await
            } else {
                UnixStream::connect(socket_path).await
            }
        }
    };

    let probe_result = timeout(
        Duration::from_secs(4),
        run_app_server_probe(
            AppServerProbeAction::WaitForReady,
            Duration::from_millis(2600),
            REMOTE_WAIT,
            connector,
        ),
    )
    .await
    .expect("one connect timeout should not reset or consume the full job budget");
    assert!(
        probe_result.is_ok(),
        "WaitForReady returned {:?} after {} connector attempts",
        probe_result.as_ref().err(),
        attempts.load(Ordering::Relaxed),
    );
    let observation = probe_result.expect("WaitForReady should retry Timeout at Connect");
    let transcript = fixture
        .await
        .expect("fixture task should join")
        .expect("fixture exchange should complete");

    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    assert_eq!(
        observation.remote_control(),
        &RemoteControlObservation::Connected {
            server_name: "ready-host".to_owned(),
            environment_id: None,
        }
    );
    assert_eq!(transcript.action_request, Some(expected_status_request()));
}

#[tokio::test]
async fn wait_for_ready_exhausts_one_absolute_connect_budget() {
    let socket =
        FixtureSocket::new("wait-budget-exhaustion").expect("fixture directory should be created");
    let attempts = Arc::new(AtomicUsize::new(0));
    let connector_attempts = Arc::clone(&attempts);
    let socket_path = socket.path().to_owned();
    let connector = move || {
        connector_attempts.fetch_add(1, Ordering::Relaxed);
        let socket_path = socket_path.clone();
        async move { UnixStream::connect(socket_path).await }
    };
    let native_wait = Duration::from_millis(90);
    let started_at = Instant::now();

    let result = timeout(
        native_wait + Duration::from_millis(250),
        run_app_server_probe(
            AppServerProbeAction::WaitForReady,
            native_wait,
            REMOTE_WAIT,
            connector,
        ),
    )
    .await
    .expect("retrying must stay within the original native budget");
    let error = result.expect_err("an unavailable socket should exhaust the connect budget");

    assert!(matches!(
        error,
        CodexProtocolError::Timeout {
            stage: NativeObservationStage::Connect,
        }
    ));
    assert!(attempts.load(Ordering::Relaxed) >= 2);
    assert!(started_at.elapsed() <= native_wait + Duration::from_millis(250));
}

#[tokio::test]
async fn wait_for_ready_returns_nonretryable_native_failures_after_one_connection() {
    let cases = [
        ("wait-websocket", FailureCase::WebSocket),
        ("wait-json", FailureCase::Json),
        ("wait-closed", FailureCase::ClosedInitialize),
        (
            "wait-invalid-response",
            FailureCase::InvalidInitializeResponse,
        ),
        ("wait-invalid-user-agent", FailureCase::InvalidUserAgent),
        ("wait-native-timeout", FailureCase::NativeReadinessTimeout),
    ];

    for (socket_name, failure_case) in cases {
        let socket = FixtureSocket::new(socket_name).expect("fixture directory should be created");
        let fixture = spawn_fixture(
            UnixListener::bind(socket.path()).expect("fixture socket should bind"),
            failure_case.plan(),
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let connector_attempts = Arc::clone(&attempts);
        let socket_path = socket.path().to_owned();
        let connector = move || {
            connector_attempts.fetch_add(1, Ordering::Relaxed);
            let socket_path = socket_path.clone();
            async move { UnixStream::connect(socket_path).await }
        };
        let native_wait = if failure_case == FailureCase::NativeReadinessTimeout {
            Duration::from_millis(40)
        } else {
            Duration::from_secs(1)
        };

        let error = timeout(
            native_wait + Duration::from_millis(250),
            run_app_server_probe(
                AppServerProbeAction::WaitForReady,
                native_wait,
                REMOTE_WAIT,
                connector,
            ),
        )
        .await
        .expect("nonretryable failures should return before a second attempt")
        .expect_err("fixture failure must remain an error");
        let transcript = fixture
            .await
            .expect("fixture task should join")
            .expect("fixture exchange should complete");

        assert!(matches_expected_failure(&error, failure_case.expected()));
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        assert!(transcript.action_request.is_none());
    }
}
