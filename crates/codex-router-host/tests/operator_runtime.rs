use std::fs::OpenOptions;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_router_host::AppServerCondition;
use codex_router_host::AppServerLaunchPlan;
use codex_router_host::ChildCommandSpec;
use codex_router_host::ChildOutput;
use codex_router_host::ExecutableRelation;
use codex_router_host::HostConfig;
use codex_router_host::HostConfigInputs;
use codex_router_host::HostCoordinationPaths;
use codex_router_host::HostDeadlineInputs;
use codex_router_host::HostDeadlines;
use codex_router_host::HostDependencies;
use codex_router_host::HostDependenciesInputs;
use codex_router_host::HostInstance;
use codex_router_host::HostOperation;
use codex_router_host::HostPhase;
use codex_router_host::HostRuntime;
use codex_router_host::HostSnapshot;
use codex_router_host::HostSnapshotDimensions;
use codex_router_host::HostedReadiness;
use codex_router_host::InstanceAcquireError;
use codex_router_host::MutationAdmission;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorProtocolError;
use codex_router_host::OperatorRequest;
use codex_router_host::RecoveryBudget;
use codex_router_host::RemoteControlCondition;
use codex_router_host::RouterCondition;
use codex_router_host::TerminalClassification;
use codex_router_host::classify_operator_request;
use codex_router_host::decode_operator_frame;
use codex_router_host::decode_operator_request;
use codex_router_host::encode_operator_frame;
use codex_router_host::encode_operator_request;
use codex_router_host::inherited_lock_marker;
use codex_router_host::send_operator_request;
use codex_router_test_support::shared_host::PersistentRouterHealthFixture;
use codex_router_test_support::shared_host::run_native_app_server_fixture;

const EXPECTED_PROTOCOL_VERSION: u16 = 1;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
fn runtime_busy_admission_keeps_status_read_only_during_mutation() {
    assert_eq!(
        classify_operator_request(
            Some(HostOperation::RestartAppServer),
            OperatorRequest::Status,
        ),
        MutationAdmission::ReadOnly
    );
    assert_eq!(
        classify_operator_request(
            Some(HostOperation::RestartAppServer),
            OperatorRequest::UpdateCodex,
        ),
        MutationAdmission::Busy
    );
    assert_eq!(
        classify_operator_request(None, OperatorRequest::RestartRouter),
        MutationAdmission::StartMutation(HostOperation::RestartRouter)
    );
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

#[tokio::test]
async fn runtime_recovery_restart_is_bounded_and_idle_is_event_driven()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("runtime-recovery")?;
    let router = PersistentRouterHealthFixture::start().await?;
    let coordination_paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let app_server_socket = directory.path().join("app-server.sock");
    let process_log = directory.path().join("app-server-pids.log");
    std::fs::write(&process_log, b"")?;
    let current_executable = std::env::current_exe()?;
    let identity = codex_router_codex::executable_identity(&current_executable).await?;
    let command = ChildCommandSpec::new(current_executable)
        .with_arguments([
            "--exact",
            "runtime_native_app_server_child_entrypoint",
            "--nocapture",
        ])
        .with_environment(
            "CODEX_ROUTER_HOST_RUNTIME_NATIVE_SOCKET",
            &app_server_socket,
        )
        .with_environment("CODEX_ROUTER_HOST_RUNTIME_NATIVE_VERSION", "1.2.3")
        .with_environment("CODEX_ROUTER_HOST_RUNTIME_PROCESS_LOG", &process_log)
        .with_output(ChildOutput::Null);
    let app_server = AppServerLaunchPlan::new(command, identity, "1.2.3".to_owned());
    let deadlines = HostDeadlines::new(HostDeadlineInputs {
        router_start: Duration::from_secs(2),
        app_server_start: Duration::from_secs(2),
        remote_control: Duration::from_secs(1),
        endpoint_inspection: Duration::from_millis(200),
        operator_request: Duration::from_secs(2),
    })?;
    let config = HostConfig::new(HostConfigInputs {
        coordination_paths: coordination_paths.clone(),
        router_endpoint: router.address(),
        app_server_socket: app_server_socket.clone(),
        managed_executable: directory.path().join("unused-managed-codex"),
        deadlines,
    });
    let dependencies = HostDependencies::new(HostDependenciesInputs {
        router_command: None,
        app_server,
    });

    let runtime = tokio::spawn(HostRuntime::run(config, dependencies));
    let initial_frames = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::AwaitHostStart,
        Duration::from_secs(3),
    )
    .await?;
    check_equal(
        terminal_snapshot(&initial_frames)?.hosted_readiness(),
        HostedReadiness::Ready,
        "host startup must reach full readiness",
    )?;
    let router_probe_count = router.request_count();
    tokio::time::sleep(Duration::from_millis(100)).await;
    check_equal(
        router.request_count(),
        router_probe_count,
        "idle runtime must not poll router health",
    )?;

    let first_processes = wait_for_process_ids(&process_log, 1).await?;
    signal_process(
        *first_processes
            .first()
            .ok_or("first app-server PID is missing")?,
    )?;
    let recovered_processes = wait_for_process_ids(&process_log, 2).await?;
    let recovery_frames = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::Status,
        Duration::from_secs(3),
    )
    .await?;
    check_equal(
        terminal_snapshot(&recovery_frames)?.recovery_budget(),
        RecoveryBudget::Consumed,
        "first unexpected exit must consume the one recovery attempt",
    )?;
    signal_process(
        *recovered_processes
            .last()
            .ok_or("recovered app-server PID is missing")?,
    )?;
    let exhausted_snapshot =
        wait_for_unavailable_status(coordination_paths.operator_socket(), Duration::from_secs(3))
            .await?;
    check_equal(
        exhausted_snapshot.recovery_budget(),
        RecoveryBudget::Consumed,
        "second unexpected exit must leave recovery exhausted",
    )?;
    check(
        matches!(
            exhausted_snapshot.app_server(),
            AppServerCondition::Absent | AppServerCondition::Failed
        ),
        "second unexpected exit must not start a third app-server",
    )?;

    let restart_frames = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::RestartAppServer,
        Duration::from_secs(3),
    )
    .await?;
    check_equal(
        terminal_classification(&restart_frames)?,
        TerminalClassification::Succeeded,
        "explicit native-ready restart must succeed after recovery exhaustion",
    )?;
    let explicitly_restarted_processes = wait_for_process_ids(&process_log, 3).await?;
    check_equal(
        terminal_snapshot(&restart_frames)?.recovery_budget(),
        RecoveryBudget::Available,
        "explicit native-ready restart must reset the recovery budget",
    )?;

    let external_router_restart = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::RestartRouter,
        Duration::from_secs(3),
    )
    .await?;
    check_equal(
        terminal_classification(&external_router_restart)?,
        TerminalClassification::Failed,
        "external router restart must report not-owned without signalling it",
    )?;
    check_equal(
        wait_for_process_ids(&process_log, 3).await?.last().copied(),
        explicitly_restarted_processes.last().copied(),
        "external router restart must not replace the app-server",
    )?;

    signal_process(
        *explicitly_restarted_processes
            .last()
            .ok_or("explicitly restarted app-server PID is missing")?,
    )?;

    runtime.abort();
    let _runtime_result = runtime.await;
    router.finish().await?;
    Ok(())
}

#[tokio::test]
async fn runtime_native_app_server_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let Some(socket_path) = std::env::var_os("CODEX_ROUTER_HOST_RUNTIME_NATIVE_SOCKET") else {
        return Ok(());
    };
    let version = std::env::var("CODEX_ROUTER_HOST_RUNTIME_NATIVE_VERSION")?;
    let process_log = std::env::var_os("CODEX_ROUTER_HOST_RUNTIME_PROCESS_LOG")
        .ok_or("runtime fixture process log is missing")?;
    run_native_app_server_fixture(
        Path::new(&socket_path),
        &version,
        Some(Path::new(&process_log)),
    )
    .await
}

fn terminal_snapshot(
    frames: &[OperatorFrame],
) -> Result<&HostSnapshot, Box<dyn std::error::Error>> {
    frames
        .iter()
        .find_map(|frame| match frame {
            OperatorFrame::Terminal(response) => Some(response.snapshot()),
            OperatorFrame::Progress(_) => None,
        })
        .ok_or_else(|| std::io::Error::other("operator response omitted its terminal frame").into())
}

fn terminal_classification(
    frames: &[OperatorFrame],
) -> Result<TerminalClassification, Box<dyn std::error::Error>> {
    frames
        .iter()
        .find_map(|frame| match frame {
            OperatorFrame::Terminal(response) => Some(response.classification()),
            OperatorFrame::Progress(_) => None,
        })
        .ok_or_else(|| std::io::Error::other("operator response omitted its terminal frame").into())
}

async fn wait_for_process_ids(
    process_log: &Path,
    expected_count: usize,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let process_ids = std::fs::read_to_string(process_log)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.parse::<u32>().ok())
                .collect::<Vec<_>>();
            if process_ids.len() >= expected_count {
                return process_ids;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}

fn signal_process(process_id: u32) -> Result<(), Box<dyn std::error::Error>> {
    let signed_process_id = i32::try_from(process_id)?;
    let process_id = rustix::process::Pid::from_raw(signed_process_id)
        .ok_or("fixture process ID must be nonzero")?;
    rustix::process::kill_process(process_id, rustix::process::Signal::KILL)?;
    Ok(())
}

async fn wait_for_unavailable_status(
    operator_socket: &Path,
    deadline: Duration,
) -> Result<HostSnapshot, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(deadline, async {
        loop {
            if let Ok(frames) = send_operator_request(
                operator_socket,
                OperatorRequest::Status,
                Duration::from_millis(500),
            )
            .await
                && let Ok(snapshot) = terminal_snapshot(&frames)
                && matches!(
                    snapshot.app_server(),
                    AppServerCondition::Absent | AppServerCondition::Failed
                )
            {
                return snapshot.clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}

#[tokio::test]
async fn live_contender_never_unlinks_and_next_owner_replaces_stale_socket()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("singleton")?;
    let paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let held_lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(paths.instance_lock())?;
    held_lock.try_lock()?;
    let stale_listener = std::os::unix::net::UnixListener::bind(paths.operator_socket())?;
    drop(stale_listener);
    let stale_inode = std::fs::metadata(paths.operator_socket())?.ino();

    let contender = HostInstance::acquire(paths.clone());
    check(
        matches!(contender, Err(InstanceAcquireError::AlreadyRunning)),
        "live singleton contender must be rejected",
    )?;
    check_equal(
        std::fs::metadata(paths.operator_socket())?.ino(),
        stale_inode,
        "contender must not unlink the existing socket",
    )?;

    held_lock.unlock()?;
    drop(held_lock);
    let owner = HostInstance::acquire(paths.clone())?;
    let rebound_inode = std::fs::metadata(paths.operator_socket())?.ino();
    check(
        rebound_inode != stale_inode,
        "new lock owner must replace the stale socket",
    )?;
    check_equal(
        std::fs::metadata(paths.operator_socket())?
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "operator socket must be owner-only",
    )?;
    drop(owner);
    check(
        !paths.operator_socket().exists(),
        "normal owner drop must remove the socket",
    )?;
    check(
        paths.instance_lock().exists(),
        "stable lock artifact must remain inert on disk",
    )?;
    Ok(())
}

#[test]
fn inherited_lock_bootstrap_validates_same_version_and_retains_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("inherited-lock")?;
    let paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let held_lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(paths.instance_lock())?;
    held_lock.try_lock()?;
    let stale_listener = std::os::unix::net::UnixListener::bind(paths.operator_socket())?;
    drop(stale_listener);

    let output = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("inherited_lock_child_entrypoint")
        .arg("--nocapture")
        .stdin(Stdio::from(held_lock.try_clone()?))
        .env("CODEX_ROUTER_HOST_TEST_INHERITED_CHILD", "1")
        .env(
            "CODEX_ROUTER_HOST_TEST_OPERATOR_SOCKET",
            paths.operator_socket(),
        )
        .env(
            "CODEX_ROUTER_HOST_TEST_INSTANCE_LOCK",
            paths.instance_lock(),
        )
        .env(
            "CODEX_ROUTER_HOST_TEST_INHERITED_MARKER",
            inherited_lock_marker(),
        )
        .output()?;

    check(
        output.status.success(),
        &format!(
            "inherited child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    check(
        !paths.operator_socket().exists(),
        "inherited owner must clean up its socket",
    )?;
    Ok(())
}

#[test]
fn prepare_lock_for_exec_uses_stdin_without_unsafe_code() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = TestDirectory::new("prepare-exec")?;
    let paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let output = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("prepare_lock_child_entrypoint")
        .arg("--nocapture")
        .env("CODEX_ROUTER_HOST_TEST_PREPARE_CHILD", "1")
        .env(
            "CODEX_ROUTER_HOST_TEST_OPERATOR_SOCKET",
            paths.operator_socket(),
        )
        .env(
            "CODEX_ROUTER_HOST_TEST_INSTANCE_LOCK",
            paths.instance_lock(),
        )
        .output()?;

    check(
        output.status.success(),
        &format!(
            "prepare child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    Ok(())
}

#[tokio::test]
async fn inherited_lock_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_TEST_INHERITED_CHILD").is_none() {
        return Ok(());
    }
    let paths = child_coordination_paths()?;
    let marker = std::env::var_os("CODEX_ROUTER_HOST_TEST_INHERITED_MARKER")
        .ok_or("inherited child marker is missing")?;

    let instance = HostInstance::acquire_inherited(paths.clone(), &marker)?;
    check(
        paths.operator_socket().exists(),
        "inherited owner must publish the operator socket",
    )?;
    drop(instance);
    check(
        !paths.operator_socket().exists(),
        "inherited owner drop must remove the operator socket",
    )?;
    Ok(())
}

#[tokio::test]
async fn prepare_lock_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_TEST_PREPARE_CHILD").is_none() {
        return Ok(());
    }
    let paths = child_coordination_paths()?;
    let instance = HostInstance::acquire(paths.clone())?;

    instance.prepare_lock_for_exec()?;

    let inherited = rustix::fs::fstat(rustix::stdio::stdin())?;
    let artifact = rustix::fs::stat(paths.instance_lock())?;
    check_equal(
        inherited.st_dev,
        artifact.st_dev,
        "prepared descriptor device must match the lock artifact",
    )?;
    check_equal(
        inherited.st_ino,
        artifact.st_ino,
        "prepared descriptor inode must match the lock artifact",
    )?;
    check(
        !rustix::io::fcntl_getfd(rustix::stdio::stdin())?.contains(rustix::io::FdFlags::CLOEXEC),
        "prepared stdin descriptor must survive the immediate exec",
    )?;
    Ok(())
}

fn child_coordination_paths() -> Result<HostCoordinationPaths, Box<dyn std::error::Error>> {
    let operator_socket = std::env::var_os("CODEX_ROUTER_HOST_TEST_OPERATOR_SOCKET")
        .ok_or("child operator socket is missing")?;
    let instance_lock = std::env::var_os("CODEX_ROUTER_HOST_TEST_INSTANCE_LOCK")
        .ok_or("child instance lock is missing")?;
    Ok(HostCoordinationPaths::new(
        PathBuf::from(operator_socket),
        PathBuf::from(instance_lock),
    ))
}

fn ready_snapshot() -> HostSnapshot {
    HostSnapshot::new(HostSnapshotDimensions {
        phase: HostPhase::Steady,
        router: RouterCondition::ExternalReachable,
        app_server: AppServerCondition::NativeReady {
            running_version: "1.2.3".to_owned(),
        },
        remote_control: RemoteControlCondition::Connected,
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

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> std::io::Result<Self> {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("crho-{name}-{}-{counter}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _cleanup_result = std::fs::remove_dir_all(&self.path);
    }
}
