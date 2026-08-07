use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_router_host::AppServerLaunchPlan;
use codex_router_host::ChildCommandSpec;
use codex_router_host::ChildOutput;
use codex_router_host::HostConfig;
use codex_router_host::HostConfigInputs;
use codex_router_host::HostCoordinationPaths;
use codex_router_host::HostDeadlineInputs;
use codex_router_host::HostDeadlines;
use codex_router_host::HostDependencies;
use codex_router_host::HostDependenciesInputs;
use codex_router_host::HostExit;
use codex_router_host::HostRuntime;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::RecoveryBudget;
use codex_router_host::TerminalClassification;
use codex_router_host::send_operator_request;
use codex_router_test_support::shared_host::run_native_app_server_fixture;
use codex_router_test_support::shared_host::run_persistent_router_health_fixture;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn owned_router_restart_replaces_only_router_and_preserves_app_server_state()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("owned-router-restart")?;
    let router_probe = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let router_address = router_probe.local_addr()?;
    drop(router_probe);
    let router_process_log = directory.path().join("router-pids.log");
    let app_server_process_log = directory.path().join("app-server-pids.log");
    std::fs::write(&router_process_log, b"")?;
    std::fs::write(&app_server_process_log, b"")?;

    let current_executable = std::env::current_exe()?;
    let app_server_socket = directory.path().join("app.sock");
    let identity = codex_router_codex::executable_identity(&current_executable).await?;
    let app_server = AppServerLaunchPlan::new(
        ChildCommandSpec::new(current_executable.clone())
            .with_arguments([
                "--exact",
                "owned_router_restart_app_server_child_entrypoint",
                "--nocapture",
            ])
            .with_environment("CODEX_HOST_RESTART_APP_SOCKET", &app_server_socket)
            .with_environment("CODEX_HOST_RESTART_APP_LOG", &app_server_process_log)
            .with_output(ChildOutput::Null),
        identity,
        "1.2.3".to_owned(),
    );
    let router_command = ChildCommandSpec::new(current_executable)
        .with_arguments([
            "--exact",
            "owned_router_restart_child_entrypoint",
            "--nocapture",
        ])
        .with_environment(
            "CODEX_HOST_RESTART_ROUTER_ADDRESS",
            router_address.to_string(),
        )
        .with_environment("CODEX_HOST_RESTART_ROUTER_LOG", &router_process_log)
        .with_output(ChildOutput::Null);
    let coordination_paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let config = HostConfig::new(HostConfigInputs {
        coordination_paths: coordination_paths.clone(),
        router_endpoint: router_address,
        app_server_socket,
        managed_executable: directory.path().join("unused-codex"),
        deadlines: HostDeadlines::new(HostDeadlineInputs {
            router_start: Duration::from_millis(500),
            app_server_start: Duration::from_secs(2),
            remote_control: Duration::from_secs(1),
            endpoint_inspection: Duration::from_millis(200),
            operator_request: Duration::from_secs(3),
        })?,
    });
    let runtime = tokio::spawn(HostRuntime::run(
        config,
        HostDependencies::new(HostDependenciesInputs {
            router_command: Some(router_command),
            app_server,
        }),
    ));

    let startup = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::AwaitHostStart,
        Duration::from_secs(4),
    )
    .await?;
    check_equal(
        terminal_classification(&startup)?,
        TerminalClassification::Ready,
        "owned-router host startup must become ready",
    )?;
    let initial_app_server_pid = wait_for_process_ids(&app_server_process_log, 1).await?[0];
    let initial_router_pid = wait_for_process_ids(&router_process_log, 1).await?[0];

    let restart = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::RestartRouter,
        Duration::from_secs(4),
    )
    .await?;
    check_equal(
        terminal_classification(&restart)?,
        TerminalClassification::Succeeded,
        "owned router restart must succeed",
    )?;
    let restarted_router_pids = wait_for_process_ids(&router_process_log, 2).await?;
    check(
        restarted_router_pids[1] != initial_router_pid,
        "owned router restart must replace the exact router child",
    )?;
    check_equal(
        wait_for_process_ids(&app_server_process_log, 1).await?[0],
        initial_app_server_pid,
        "router restart must not replace the app-server",
    )?;
    check_equal(
        terminal_snapshot(&restart)?.recovery_budget(),
        RecoveryBudget::Available,
        "router restart must not consume app-server recovery",
    )?;

    terminate_process(restarted_router_pids[1])?;
    check_equal(
        runtime.await??,
        HostExit::OwnedRouterExited,
        "unexpected owned-router exit must stop the foreground host",
    )?;
    Ok(())
}

#[tokio::test]
async fn owned_router_restart_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let Some(address) = std::env::var_os("CODEX_HOST_RESTART_ROUTER_ADDRESS") else {
        return Ok(());
    };
    let process_log = std::env::var_os("CODEX_HOST_RESTART_ROUTER_LOG")
        .ok_or("owned router process log is missing")?;
    run_persistent_router_health_fixture(
        address.to_string_lossy().parse::<SocketAddr>()?,
        Path::new(&process_log),
    )
    .await
}

#[tokio::test]
async fn owned_router_restart_app_server_child_entrypoint() -> Result<(), Box<dyn std::error::Error>>
{
    let Some(socket_path) = std::env::var_os("CODEX_HOST_RESTART_APP_SOCKET") else {
        return Ok(());
    };
    let process_log = std::env::var_os("CODEX_HOST_RESTART_APP_LOG")
        .ok_or("owned restart app-server process log is missing")?;
    run_native_app_server_fixture(
        Path::new(&socket_path),
        "1.2.3",
        Some(Path::new(&process_log)),
    )
    .await
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
        .ok_or_else(|| std::io::Error::other("operator terminal response is missing").into())
}

fn terminal_snapshot(
    frames: &[OperatorFrame],
) -> Result<&codex_router_host::HostSnapshot, Box<dyn std::error::Error>> {
    frames
        .iter()
        .find_map(|frame| match frame {
            OperatorFrame::Terminal(response) => Some(response.snapshot()),
            OperatorFrame::Progress(_) => None,
        })
        .ok_or_else(|| std::io::Error::other("operator terminal response is missing").into())
}

async fn wait_for_process_ids(
    process_log: &Path,
    expected_count: usize,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(4), async {
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

fn terminate_process(process_id: u32) -> Result<(), Box<dyn std::error::Error>> {
    let process_id = rustix::process::Pid::from_raw(i32::try_from(process_id)?)
        .ok_or("fixture process ID must be nonzero")?;
    rustix::process::kill_process(process_id, rustix::process::Signal::TERM)?;
    Ok(())
}

fn check_equal<TValue>(
    actual: TValue,
    expected: TValue,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>>
where
    TValue: PartialEq,
{
    if actual == expected {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

fn check(condition: bool, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> std::io::Result<Self> {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("crhr-{name}-{}-{counter}", std::process::id()));
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
