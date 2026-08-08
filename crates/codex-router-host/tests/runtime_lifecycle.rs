use std::path::Path;
use std::path::PathBuf;
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
use codex_router_host::HostPhase;
use codex_router_host::HostRuntime;
use codex_router_host::HostSnapshot;
use codex_router_host::ManagedChildLaunchPlans;
use codex_router_host::ManagedUpdateInputs;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::RecoveryBudget;
use codex_router_host::RouterCondition;
use codex_router_host::TerminalClassification;
use codex_router_test_support::native_app_server::run_native_app_server_fixture;
use codex_router_test_support::router_health::PersistentRouterHealthFixture;

#[path = "support/operator_client.rs"]
mod operator_client;
use operator_client::send_operator_request;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    let managed_executable = directory.path().join("managed-codex");
    write_managed_codex_version_fixture(&managed_executable, "1.2.3")?;
    let current_executable = std::env::current_exe()?;
    let identity = codex_router_codex::executable_identity(&managed_executable).await?;
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
        .with_environment(
            "CODEX_ROUTER_HOST_RUNTIME_MANAGED_EXECUTABLE",
            &managed_executable,
        )
        .with_environment("CODEX_ROUTER_HOST_RUNTIME_PROCESS_LOG", &process_log)
        .with_output(ChildOutput::Null);
    let app_server = AppServerLaunchPlan::new(command, identity, "1.2.3".to_owned());
    let deadlines = HostDeadlines::new(HostDeadlineInputs {
        router_start: Duration::from_secs(2),
        app_server_start: Duration::from_secs(15),
        remote_control: Duration::from_secs(1),
        endpoint_inspection: Duration::from_millis(200),
        operator_request: Duration::from_secs(15),
    })?;
    let config = HostConfig::new(HostConfigInputs {
        coordination_paths: coordination_paths.clone(),
        router_endpoint: router.address(),
        app_server_socket: app_server_socket.clone(),
        managed_executable: managed_executable.clone(),
        deadlines,
    });
    let child_launch_plans = ManagedChildLaunchPlans::new(None, app_server);

    let runtime = tokio::spawn(HostRuntime::run(
        config,
        child_launch_plans,
        ManagedUpdateInputs::production(),
    ));
    let initial_frames = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::AwaitHostStart,
        Duration::from_secs(20),
    )
    .await?;
    check_equal(
        terminal_snapshot(&initial_frames)?.hosted_readiness(),
        codex_router_host::HostedReadiness::Ready,
        "host startup must reach full readiness",
    )?;
    write_managed_codex_version_fixture(&managed_executable, "2.0.0")?;
    let drift_frames = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::Status,
        Duration::from_secs(20),
    )
    .await?;
    check_equal(
        terminal_snapshot(&drift_frames)?.executable_relation(),
        ExecutableRelation::Drift,
        "status must compare the running child identity with the installed executable",
    )?;
    let router_probe_count = router.request_count();
    tokio::time::sleep(Duration::from_millis(100)).await;
    check_equal(
        router.request_count(),
        router_probe_count,
        "idle runtime must not poll router health",
    )?;
    router.finish().await?;
    let router_unavailable = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::Status,
        Duration::from_secs(20),
    )
    .await?;
    check_equal(
        terminal_snapshot(&router_unavailable)?.router(),
        RouterCondition::Unavailable,
        "status must observe that an external router stopped after startup",
    )?;
    check_equal(
        terminal_snapshot(&router_unavailable)?.hosted_readiness(),
        codex_router_host::HostedReadiness::Unavailable,
        "status must not report hosted readiness after router loss",
    )?;

    let first_processes = wait_for_process_ids(&process_log, 1).await?;
    signal_process(
        *first_processes
            .first()
            .ok_or("first app-server PID is missing")?,
    )?;
    let recovered_processes = wait_for_process_ids(&process_log, 2).await?;
    let recovery_frames = wait_for_steady_status(
        coordination_paths.operator_socket(),
        Duration::from_secs(20),
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
    let exhausted_snapshot = wait_for_unavailable_status(
        coordination_paths.operator_socket(),
        Duration::from_secs(20),
    )
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
        Duration::from_secs(20),
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
        Duration::from_secs(20),
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
    Ok(())
}

async fn wait_for_steady_status(
    operator_socket: &Path,
    deadline: Duration,
) -> Result<Vec<OperatorFrame>, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(deadline, async {
        loop {
            if let Ok(frames) = send_operator_request(
                operator_socket,
                OperatorRequest::Status,
                Duration::from_secs(1),
            )
            .await
                && terminal_snapshot(&frames)
                    .is_ok_and(|snapshot| matches!(snapshot.phase(), HostPhase::Steady))
            {
                return frames;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}

#[tokio::test]
async fn runtime_native_app_server_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let Some(socket_path) = std::env::var_os("CODEX_ROUTER_HOST_RUNTIME_NATIVE_SOCKET") else {
        return Ok(());
    };
    let managed_executable = std::env::var_os("CODEX_ROUTER_HOST_RUNTIME_MANAGED_EXECUTABLE")
        .ok_or("runtime fixture managed executable is missing")?;
    let version =
        codex_router_codex::managed_executable_version(Path::new(&managed_executable)).await?;
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
    Ok(tokio::time::timeout(Duration::from_secs(20), async {
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

fn write_managed_codex_version_fixture(
    executable: &Path,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(
        executable,
        format!("#!/bin/sh\necho 'codex-cli {version}'\n"),
    )?;
    let mut permissions = std::fs::metadata(executable)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(executable, permissions)?;
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
