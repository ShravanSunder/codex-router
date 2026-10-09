use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_native_integration::AppServerProbeAction;
use codex_native_integration::NativeObservationStage;
use codex_native_integration::RemoteControlObservation;
use codex_native_integration::observe_app_server;
use codex_native_integration::run_app_server_probe;
use serde_json::json;
use tokio::net::UnixListener;
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::time::timeout;

#[path = "support/native_probe_fixture.rs"]
#[allow(dead_code)] // The shared fixture also supplies readiness-only scenarios.
mod native_probe_fixture;
use native_probe_fixture::{
    ActionBehavior, ExpectedFailure, FIXTURE_TIMEOUT, FixturePlan, FixtureSocket, PeerEnd,
    REMOTE_WAIT, expected_enable_request, expected_initialize_request, expected_status_request,
    matches_expected_failure, spawn_fixture, status,
};

#[tokio::test]
async fn observe_action_uses_one_status_read_without_enabling_remote_control() {
    let socket = FixtureSocket::new("observe-action").expect("fixture directory should be created");
    let fixture = spawn_fixture(
        UnixListener::bind(socket.path()).expect("fixture socket should bind"),
        FixturePlan::respond_then_change(
            status("connected", "owner-mac", Some("env_123")),
            status("errored", "later-error", None),
        ),
    );
    let attempts = Arc::new(AtomicUsize::new(0));
    let connector_attempts = Arc::clone(&attempts);
    let socket_path = socket.path().to_owned();
    let connector = move || {
        connector_attempts.fetch_add(1, Ordering::Relaxed);
        let socket_path = socket_path.clone();
        async move { UnixStream::connect(socket_path).await }
    };

    let observation = run_app_server_probe(
        AppServerProbeAction::Observe,
        Duration::from_secs(1),
        REMOTE_WAIT,
        connector,
    )
    .await
    .expect("Observe should decode the status response");
    let transcript = fixture
        .await
        .expect("fixture task should join")
        .expect("fixture exchange should complete");

    assert_eq!(attempts.load(Ordering::Relaxed), 1);
    assert_eq!(observation.running_version(), "1.2.3");
    assert_eq!(
        observation.remote_control(),
        &RemoteControlObservation::Connected {
            server_name: "owner-mac".to_owned(),
            environment_id: Some("env_123".to_owned()),
        }
    );
    assert_eq!(
        transcript.initialize_request,
        Some(expected_initialize_request())
    );
    assert_eq!(
        transcript.initialized_notification,
        Some(json!({"method": "initialized"}))
    );
    assert_eq!(transcript.action_request, Some(expected_status_request()));
    assert!(matches!(transcript.peer_end, Some(PeerEnd::CloseFrame)));
}

#[tokio::test]
async fn enable_and_observe_sends_one_ephemeral_request_and_reads_its_result() {
    let socket =
        FixtureSocket::new("enable-connected").expect("fixture directory should be created");
    let fixture = spawn_fixture(
        UnixListener::bind(socket.path()).expect("fixture socket should bind"),
        FixturePlan::respond_then_change(
            status("connected", "owner-mac", Some("env_123")),
            status("errored", "later-error", None),
        ),
    );
    let attempts = Arc::new(AtomicUsize::new(0));
    let connector_attempts = Arc::clone(&attempts);
    let socket_path = socket.path().to_owned();
    let connector = move || {
        connector_attempts.fetch_add(1, Ordering::Relaxed);
        let socket_path = socket_path.clone();
        async move { UnixStream::connect(socket_path).await }
    };

    let observation = run_app_server_probe(
        AppServerProbeAction::EnableAndObserve,
        Duration::from_secs(1),
        REMOTE_WAIT,
        connector,
    )
    .await
    .expect("EnableAndObserve should decode the enable response");
    let transcript = fixture
        .await
        .expect("fixture task should join")
        .expect("fixture exchange should complete");

    assert_eq!(attempts.load(Ordering::Relaxed), 1);
    assert_eq!(
        observation.remote_control(),
        &RemoteControlObservation::Connected {
            server_name: "owner-mac".to_owned(),
            environment_id: Some("env_123".to_owned()),
        }
    );
    assert_eq!(
        transcript.initialize_request,
        Some(expected_initialize_request())
    );
    assert_eq!(
        transcript.initialized_notification,
        Some(json!({"method": "initialized"}))
    );
    assert_eq!(transcript.action_request, Some(expected_enable_request()));
    assert!(matches!(transcript.peer_end, Some(PeerEnd::CloseFrame)));
}

#[tokio::test]
async fn enable_waits_for_status_change_only_after_a_connecting_response() {
    let socket =
        FixtureSocket::new("enable-status-change").expect("fixture directory should be created");
    let fixture = spawn_fixture(
        UnixListener::bind(socket.path()).expect("fixture socket should bind"),
        FixturePlan::respond_then_change(
            status("connecting", "starting-host", Some("env_starting")),
            status("connected", "ready-host", Some("env_ready")),
        ),
    );
    let attempts = Arc::new(AtomicUsize::new(0));
    let connector_attempts = Arc::clone(&attempts);
    let socket_path = socket.path().to_owned();
    let connector = move || {
        connector_attempts.fetch_add(1, Ordering::Relaxed);
        let socket_path = socket_path.clone();
        async move { UnixStream::connect(socket_path).await }
    };

    let observation = run_app_server_probe(
        AppServerProbeAction::EnableAndObserve,
        Duration::from_secs(1),
        REMOTE_WAIT,
        connector,
    )
    .await
    .expect("EnableAndObserve should observe the status change");
    let transcript = fixture
        .await
        .expect("fixture task should join")
        .expect("fixture exchange should complete");

    assert_eq!(attempts.load(Ordering::Relaxed), 1);
    assert_eq!(
        observation.remote_control(),
        &RemoteControlObservation::Connected {
            server_name: "ready-host".to_owned(),
            environment_id: Some("env_ready".to_owned()),
        }
    );
    assert_eq!(transcript.action_request, Some(expected_enable_request()));
    assert!(matches!(transcript.peer_end, Some(PeerEnd::CloseFrame)));
}

#[tokio::test]
async fn observe_and_enable_share_the_whole_remote_window_before_initial_status() {
    for (action, use_legacy_observer) in [
        (AppServerProbeAction::Observe, true),
        (AppServerProbeAction::EnableAndObserve, false),
    ] {
        let socket = FixtureSocket::new("remote-window-before-status")
            .expect("fixture directory should be created");
        let fixture = spawn_fixture(
            UnixListener::bind(socket.path()).expect("fixture socket should bind"),
            FixturePlan::respond_after(
                status("connecting", "late-host", Some("late-env")),
                Duration::from_millis(100),
            ),
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let connector_attempts = Arc::clone(&attempts);
        let socket_path = socket.path().to_owned();
        let connector = move || {
            connector_attempts.fetch_add(1, Ordering::Relaxed);
            let socket_path = socket_path.clone();
            async move { UnixStream::connect(socket_path).await }
        };

        let observation = if use_legacy_observer {
            observe_app_server(
                socket.path(),
                Duration::from_secs(1),
                Duration::from_millis(25),
            )
            .await
        } else {
            run_app_server_probe(
                action,
                Duration::from_secs(1),
                Duration::from_millis(25),
                connector,
            )
            .await
        }
        .expect("remote observation should return when its whole window expires");
        let transcript = fixture
            .await
            .expect("fixture task should join")
            .expect("fixture exchange should complete");

        if action == AppServerProbeAction::EnableAndObserve {
            assert_eq!(attempts.load(Ordering::Relaxed), 1);
            assert_eq!(transcript.action_request, Some(expected_enable_request()));
        } else {
            assert_eq!(transcript.action_request, Some(expected_status_request()));
        }
        assert_eq!(
            observation.remote_control(),
            &RemoteControlObservation::Connecting {
                server_name: "unknown".to_owned(),
                environment_id: None,
            }
        );
        assert!(!transcript.delayed_response_sent);
        assert!(matches!(transcript.peer_end, Some(PeerEnd::CloseFrame)));
    }
}

#[tokio::test]
async fn enable_and_observe_preserves_errored_and_disabled_statuses() {
    let cases = [
        (
            "enable-errored-result",
            status("errored", "owner-mac", Some("env_error")),
            RemoteControlObservation::Errored {
                server_name: "owner-mac".to_owned(),
                environment_id: Some("env_error".to_owned()),
            },
        ),
        (
            "enable-disabled-result",
            status("disabled", "owner-mac", None),
            RemoteControlObservation::Disabled {
                server_name: "owner-mac".to_owned(),
                environment_id: None,
            },
        ),
    ];

    for (socket_name, result, expected_observation) in cases {
        let socket = FixtureSocket::new(socket_name).expect("fixture directory should be created");
        let fixture = spawn_fixture(
            UnixListener::bind(socket.path()).expect("fixture socket should bind"),
            FixturePlan::respond_with(result),
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let connector_attempts = Arc::clone(&attempts);
        let socket_path = socket.path().to_owned();
        let connector = move || {
            connector_attempts.fetch_add(1, Ordering::Relaxed);
            let socket_path = socket_path.clone();
            async move { UnixStream::connect(socket_path).await }
        };

        let observation = run_app_server_probe(
            AppServerProbeAction::EnableAndObserve,
            Duration::from_secs(1),
            REMOTE_WAIT,
            connector,
        )
        .await
        .expect("known non-connecting statuses should return directly");
        let transcript = fixture
            .await
            .expect("fixture task should join")
            .expect("fixture exchange should complete");

        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        assert_eq!(observation.remote_control(), &expected_observation);
        assert_eq!(transcript.action_request, Some(expected_enable_request()));
        assert!(matches!(transcript.peer_end, Some(PeerEnd::CloseFrame)));
    }
}

#[tokio::test]
async fn enable_errors_and_malformed_results_are_failures_without_retry_or_fallback() {
    let cases = [
        (
            "enable-error-envelope",
            ActionBehavior::MissingResult,
            ExpectedFailure::InvalidResponse(NativeObservationStage::RemoteControlEnable),
        ),
        (
            "enable-malformed-json",
            ActionBehavior::MalformedJson,
            ExpectedFailure::Json,
        ),
        (
            "enable-unknown-status",
            ActionBehavior::Respond {
                result: status("future-status", "owner-mac", None),
                changed: None,
                observed: None,
            },
            ExpectedFailure::Json,
        ),
        (
            "enable-closed-peer",
            ActionBehavior::Close,
            ExpectedFailure::Closed(NativeObservationStage::RemoteControlEnable),
        ),
    ];

    for (socket_name, action_behavior, expected_failure) in cases {
        let socket = FixtureSocket::new(socket_name).expect("fixture directory should be created");
        let fixture = spawn_fixture(
            UnixListener::bind(socket.path()).expect("fixture socket should bind"),
            FixturePlan::with_action(action_behavior),
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let connector_attempts = Arc::clone(&attempts);
        let socket_path = socket.path().to_owned();
        let connector = move || {
            connector_attempts.fetch_add(1, Ordering::Relaxed);
            let socket_path = socket_path.clone();
            async move { UnixStream::connect(socket_path).await }
        };

        let error = run_app_server_probe(
            AppServerProbeAction::EnableAndObserve,
            Duration::from_secs(1),
            REMOTE_WAIT,
            connector,
        )
        .await
        .expect_err("an invalid enable result must not become a Disabled success");
        let transcript = fixture
            .await
            .expect("fixture task should join")
            .expect("fixture exchange should complete");

        assert!(
            matches_expected_failure(&error, expected_failure),
            "{socket_name}: unexpected native error: {error:?}"
        );
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        assert_eq!(transcript.action_request, Some(expected_enable_request()));
        assert!(!matches!(
            transcript.peer_end,
            Some(PeerEnd::UnexpectedMessage(_))
        ));
    }
}

#[tokio::test]
async fn canceling_enable_drops_its_exchange_without_a_second_connection() {
    let socket = FixtureSocket::new("enable-cancel").expect("fixture directory should be created");
    let (action_seen_sender, action_seen_receiver) = oneshot::channel();
    let fixture = spawn_fixture(
        UnixListener::bind(socket.path()).expect("fixture socket should bind"),
        FixturePlan::hold_action(action_seen_sender),
    );
    let attempts = Arc::new(AtomicUsize::new(0));
    let connector_attempts = Arc::clone(&attempts);
    let socket_path = socket.path().to_owned();
    let connector = move || {
        connector_attempts.fetch_add(1, Ordering::Relaxed);
        let socket_path = socket_path.clone();
        async move { UnixStream::connect(socket_path).await }
    };
    let probe_task = tokio::spawn(run_app_server_probe(
        AppServerProbeAction::EnableAndObserve,
        Duration::from_secs(2),
        Duration::from_secs(2),
        connector,
    ));

    timeout(FIXTURE_TIMEOUT, action_seen_receiver)
        .await
        .expect("fixture should receive the one enable request")
        .expect("fixture signal should be sent");
    probe_task.abort();
    assert!(
        probe_task
            .await
            .expect_err("probe task is canceled")
            .is_cancelled()
    );
    let transcript = fixture
        .await
        .expect("fixture task should join")
        .expect("fixture should observe the canceled peer");

    assert_eq!(attempts.load(Ordering::Relaxed), 1);
    assert_eq!(transcript.action_request, Some(expected_enable_request()));
    assert!(matches!(
        transcript.peer_end,
        Some(PeerEnd::Closed | PeerEnd::CloseFrame)
    ));
}
