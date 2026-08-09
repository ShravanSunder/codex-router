use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::time::Duration;

use codex_router_host::AppServerCondition;
use codex_router_host::ExecutableRelation;
use codex_router_host::HostConfig;
use codex_router_host::HostConfigInputs;
use codex_router_host::HostCoordinationPaths;
use codex_router_host::HostDeadlines;
use codex_router_host::HostOperation;
use codex_router_host::HostPhase;
use codex_router_host::HostSnapshot;
use codex_router_host::HostSnapshotDimensions;
use codex_router_host::HostedReadiness;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorProtocolError;
use codex_router_host::OperatorRequest;
use codex_router_host::RecoveryBudget;
use codex_router_host::RemoteControlCondition;
use codex_router_host::RouterCondition;
use codex_router_host::TerminalClassification;
use codex_router_host::decode_operator_frame;
use codex_router_host::decode_operator_request;
use codex_router_host::encode_operator_frame;
use codex_router_host::encode_operator_request;

const EXPECTED_PROTOCOL_VERSION: u16 = 1;

#[test]
fn protocol_is_versioned_bounded_and_accepts_exactly_one_request()
-> Result<(), Box<dyn std::error::Error>> {
    let encoded = encode_operator_request(&OperatorRequest::Status)?;
    check_equal(
        decode_operator_request(&encoded)?,
        OperatorRequest::Status,
        "encoded status request must round trip",
    )?;

    let mismatched = b"{\"protocol_version\":99,\"request\":\"status\"}\n";
    check(
        matches!(
            decode_operator_request(mismatched),
            Err(OperatorProtocolError::VersionMismatch {
                expected: EXPECTED_PROTOCOL_VERSION,
                actual: 99,
            })
        ),
        "mismatched protocol version must fail closed",
    )?;

    let mut oversized = vec![b' '; 64 * 1024];
    oversized.push(b'\n');
    check(
        matches!(
            decode_operator_request(&oversized),
            Err(OperatorProtocolError::FrameTooLarge)
        ),
        "oversized protocol frame must fail closed",
    )?;

    let multiple = b"{\"protocol_version\":1,\"request\":\"status\"}\n{\"protocol_version\":1,\"request\":\"restart_app_server\"}\n";
    check(
        matches!(
            decode_operator_request(multiple),
            Err(OperatorProtocolError::MultipleRequests)
        ),
        "multiple requests on one connection must fail closed",
    )?;
    Ok(())
}

#[test]
fn request_mutability_supports_immediate_busy_classification() {
    assert!(!OperatorRequest::Status.is_mutating());
    assert!(!OperatorRequest::AwaitHostStart.is_mutating());
    assert!(OperatorRequest::RestartAppServer.is_mutating());
    assert!(OperatorRequest::UpdateCodex.is_mutating());
    assert!(OperatorRequest::RestartRouter.is_mutating());
}

#[test]
fn runtime_startup_deadlines_compose_to_thirty_seconds() {
    assert_eq!(
        HostDeadlines::production().startup_total(),
        Duration::from_secs(30)
    );
}

#[test]
fn terminal_frames_preserve_busy_classification_and_live_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let snapshot = ready_snapshot();
    let frame = OperatorFrame::busy(
        OperatorRequest::RestartAppServer,
        snapshot.clone(),
        "another lifecycle mutation is active".to_owned(),
    );

    let encoded = encode_operator_frame(&frame)?;
    let decoded = decode_operator_frame(&encoded)?;

    check_equal(decoded.clone(), frame, "operator frame must round trip")?;
    let OperatorFrame::Terminal(response) = decoded else {
        return Err("busy response must be terminal".into());
    };
    check_equal(
        response.classification(),
        TerminalClassification::Busy,
        "overlapping mutation must return busy",
    )?;
    check_equal(
        response.snapshot(),
        &snapshot,
        "busy result must carry its live snapshot",
    )?;
    Ok(())
}

#[test]
fn hosted_readiness_is_derived_from_orthogonal_dimensions() {
    let degraded = HostSnapshot::new(HostSnapshotDimensions {
        phase: HostPhase::Mutating {
            operation: HostOperation::RestartAppServer,
            phase: "remote-control-convergence".to_owned(),
        },
        router: RouterCondition::ExternalReachable,
        app_server: AppServerCondition::NativeReady {
            running_version: "1.2.3".to_owned(),
        },
        remote_control: RemoteControlCondition::Connecting,
        remote_control_identity: None,
        executable_relation: ExecutableRelation::Match,
        recovery_budget: RecoveryBudget::Available,
        last_lifecycle_outcome: None,
    });
    assert_eq!(
        degraded.hosted_readiness(),
        HostedReadiness::LocalReadyRemoteDegraded
    );
    assert_eq!(degraded.recovery_budget(), RecoveryBudget::Available);

    let unavailable = HostSnapshot::new(HostSnapshotDimensions {
        phase: HostPhase::Steady,
        router: RouterCondition::Unavailable,
        app_server: AppServerCondition::NativeReady {
            running_version: "1.2.3".to_owned(),
        },
        remote_control: RemoteControlCondition::Connected,
        remote_control_identity: None,
        executable_relation: ExecutableRelation::Unknown,
        recovery_budget: RecoveryBudget::Consumed,
        last_lifecycle_outcome: None,
    });
    assert_eq!(unavailable.hosted_readiness(), HostedReadiness::Unavailable);
    assert_eq!(unavailable.recovery_budget(), RecoveryBudget::Consumed);
}

#[test]
fn host_config_preserves_resolved_router_and_codex_boundaries() {
    let debug_paths = HostCoordinationPaths::new(
        PathBuf::from("/debug-router/host.sock"),
        PathBuf::from("/debug-router/host.lock"),
    );
    let installed_paths = HostCoordinationPaths::new(
        PathBuf::from("/installed-router/host.sock"),
        PathBuf::from("/installed-router/host.lock"),
    );
    let explicit_paths = HostCoordinationPaths::new(
        PathBuf::from("/explicit-router/host.sock"),
        PathBuf::from("/explicit-router/host.lock"),
    );
    let app_server_socket = PathBuf::from("/normal-codex/app-server.sock");
    let managed_executable = PathBuf::from("/normal-codex/current/codex");
    let router_endpoint = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8787));

    let debug = HostConfig::new(HostConfigInputs {
        coordination_paths: debug_paths,
        router_endpoint,
        app_server_socket: app_server_socket.clone(),
        managed_executable: managed_executable.clone(),
        deadlines: HostDeadlines::production(),
    });
    let installed = HostConfig::new(HostConfigInputs {
        coordination_paths: installed_paths,
        router_endpoint,
        app_server_socket: app_server_socket.clone(),
        managed_executable: managed_executable.clone(),
        deadlines: HostDeadlines::production(),
    });
    let explicit = HostConfig::new(HostConfigInputs {
        coordination_paths: explicit_paths,
        router_endpoint,
        app_server_socket,
        managed_executable,
        deadlines: HostDeadlines::production(),
    });

    assert_ne!(debug.coordination_paths(), installed.coordination_paths());
    assert_ne!(
        installed.coordination_paths(),
        explicit.coordination_paths()
    );
    assert_eq!(debug.app_server_socket(), installed.app_server_socket());
    assert_eq!(installed.app_server_socket(), explicit.app_server_socket());
    assert_eq!(debug.managed_executable(), installed.managed_executable());
}

fn ready_snapshot() -> HostSnapshot {
    HostSnapshot::new(HostSnapshotDimensions {
        phase: HostPhase::Steady,
        router: RouterCondition::ExternalReachable,
        app_server: AppServerCondition::NativeReady {
            running_version: "1.2.3".to_owned(),
        },
        remote_control: RemoteControlCondition::Connected,
        remote_control_identity: None,
        executable_relation: ExecutableRelation::Match,
        recovery_budget: RecoveryBudget::Available,
        last_lifecycle_outcome: None,
    })
}

fn check(condition: bool, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

fn check_equal<TValue>(
    actual: TValue,
    expected: TValue,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>>
where
    TValue: PartialEq,
{
    check(actual == expected, message)
}
