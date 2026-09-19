use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_router_core::router_compatibility::RouterCompatibility;
use codex_router_host::AppServerLaunchPlan;
use codex_router_host::ChildCommandSpec;
use codex_router_host::ChildOutput;
use codex_router_host::HostConfig;
use codex_router_host::HostConfigInputs;
use codex_router_host::HostCoordinationPaths;
use codex_router_host::HostDeadlineInputs;
use codex_router_host::HostDeadlines;
use codex_router_host::HostExit;
use codex_router_host::HostInstance;
use codex_router_host::HostRuntime;
use codex_router_host::ManagedChildLaunchPlans;
use codex_router_host::ManagedUpdateInputs;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::TerminalClassification;
use codex_router_test_support::native_app_server::run_native_app_server_fixture;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

#[path = "support/operator_client.rs"]
mod operator_client;
use operator_client::send_operator_request;

#[path = "support/host_replacement_cleanup.rs"]
mod host_replacement_cleanup;
use host_replacement_cleanup::finish_fixture_proof;
use host_replacement_cleanup::reap_fixture_host;
use host_replacement_cleanup::terminate_logged_fixture_processes;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn retained_router_teardown_failure_keeps_restart_busy_and_never_executes()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("host-replacement-teardown")?;
    let router_probe = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let router_address = router_probe.local_addr()?;
    drop(router_probe);
    let router_log = directory.path().join("router.log");
    let app_server_log = directory.path().join("app-server.log");
    std::fs::write(&router_log, b"")?;
    std::fs::write(&app_server_log, b"")?;
    let invalid_executable = directory.path().join("invalid-codex-router");
    std::fs::write(&invalid_executable, b"#!/missing/interpreter\n")?;
    std::fs::set_permissions(&invalid_executable, std::fs::Permissions::from_mode(0o700))?;

    let current_executable = std::env::current_exe()?;
    let app_server_socket = directory.path().join("app.sock");
    let identity = codex_native_integration::executable_identity(&current_executable).await?;
    let app_server = AppServerLaunchPlan::new(
        ChildCommandSpec::new(current_executable.clone())
            .with_arguments([
                "--exact",
                "host_replacement_failure_app_server_entrypoint",
                "--nocapture",
            ])
            .with_environment("CODEX_HOST_FAILURE_APP_SOCKET", &app_server_socket)
            .with_environment("CODEX_HOST_FAILURE_APP_LOG", &app_server_log)
            .with_output(ChildOutput::Null),
        identity,
        "1.2.3".to_owned(),
    );
    let router_command = ChildCommandSpec::new(current_executable)
        .with_arguments([
            "--exact",
            "host_replacement_ignoring_router_entrypoint",
            "--nocapture",
        ])
        .with_environment(
            "CODEX_HOST_FAILURE_ROUTER_ADDRESS",
            router_address.to_string(),
        )
        .with_environment("CODEX_HOST_FAILURE_ROUTER_LOG", &router_log)
        .with_output(ChildOutput::Null);
    let coordination_paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let runtime = tokio::spawn(HostRuntime::run(
        HostConfig::new(HostConfigInputs {
            coordination_paths: coordination_paths.clone(),
            router_endpoint: router_address,
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            app_server_socket,
            managed_executable: directory.path().join("unused-managed-codex"),
            deadlines: fixture_host_deadlines()?,
        }),
        ManagedChildLaunchPlans::new(Some(router_command), app_server),
        ManagedUpdateInputs::production().with_replacement_command(ChildCommandSpec::new(
            PathBuf::from("/configured/old/codex-router"),
        )),
    ));

    let proof_result: Result<(), Box<dyn std::error::Error>> = async {
        let startup = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::AwaitHostStart,
            Duration::from_secs(20),
        )
        .await?;
        check_equal(
            terminal_classification(&startup)?,
            TerminalClassification::Ready,
            "teardown fixture Host must start ready",
        )?;
        let router_process_id = wait_for_router_signal(&router_log, false).await?;
        let app_server_process_id = wait_for_process_id(&app_server_log).await?;
        let first_socket = coordination_paths.operator_socket().to_owned();
        let first_executable = invalid_executable.clone();
        let first_restart = tokio::spawn(async move {
            send_operator_request(
                &first_socket,
                OperatorRequest::RestartHost {
                    executable: first_executable,
                },
                Duration::from_secs(20),
            )
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))
        });
        let observed_router_process_id = wait_for_router_signal(&router_log, true).await?;
        check_equal(
            observed_router_process_id,
            router_process_id,
            "Host replacement must signal the exact retained router",
        )?;

        let overlapping_restart = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::RestartHost {
                executable: invalid_executable,
            },
            Duration::from_secs(2),
        )
        .await?;
        check_equal(
            terminal_classification(&overlapping_restart)?,
            TerminalClassification::Busy,
            "retained teardown must continue owning mutation admission",
        )?;
        let first_frames = tokio::time::timeout(Duration::from_secs(15), first_restart).await???;
        check_equal(
            terminal_classification(&first_frames)?,
            TerminalClassification::Failed,
            "retained router timeout must fail Host replacement",
        )?;
        check(
            matches!(first_frames.first(), Some(OperatorFrame::Progress(_))),
            "teardown failure must follow replacement-starting progress",
        )?;
        check(
            process_is_running(router_process_id),
            "timed-out router must remain retained instead of being forgotten",
        )?;
        check(
            !process_is_running(app_server_process_id),
            "Host replacement must settle the app-server before router teardown",
        )
    }
    .await;

    runtime.abort();
    let _runtime_result = runtime.await;
    let cleanup_result =
        terminate_logged_fixture_processes(&[router_log.as_path(), app_server_log.as_path()]);
    finish_fixture_proof(proof_result, [cleanup_result])
}

#[tokio::test]
async fn signal_owns_shutdown_while_host_replacement_teardown_is_retained()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("host-replacement-signal")?;
    let router_probe = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let router_address = router_probe.local_addr()?;
    drop(router_probe);
    let coordination_paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let router_log = directory.path().join("router.log");
    let app_server_log = directory.path().join("app-server.log");
    let app_server_socket = directory.path().join("app.sock");
    std::fs::write(&router_log, b"")?;
    std::fs::write(&app_server_log, b"")?;
    let invalid_executable = directory.path().join("invalid-codex-router");
    std::fs::write(&invalid_executable, b"#!/missing/interpreter\n")?;
    std::fs::set_permissions(&invalid_executable, std::fs::Permissions::from_mode(0o700))?;
    let current_executable = std::env::current_exe()?;
    let mut host_process = tokio::process::Command::new(&current_executable);
    host_process
        .args([
            "--exact",
            "host_replacement_signal_child_entrypoint",
            "--nocapture",
        ])
        .env("CODEX_HOST_SIGNAL_CHILD", "1")
        .env(
            "CODEX_HOST_SIGNAL_OPERATOR_SOCKET",
            coordination_paths.operator_socket(),
        )
        .env(
            "CODEX_HOST_SIGNAL_INSTANCE_LOCK",
            coordination_paths.instance_lock(),
        )
        .env(
            "CODEX_HOST_SIGNAL_ROUTER_ADDRESS",
            router_address.to_string(),
        )
        .env("CODEX_HOST_SIGNAL_ROUTER_LOG", &router_log)
        .env("CODEX_HOST_SIGNAL_APP_SOCKET", &app_server_socket)
        .env("CODEX_HOST_SIGNAL_APP_LOG", &app_server_log)
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut host_process = host_process.spawn()?;
    let host_process_id = host_process.id();

    let proof_result: Result<(), Box<dyn std::error::Error>> = async {
        let host_process_id = host_process_id.ok_or("fixture Host PID is missing")?;
        let startup = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::AwaitHostStart,
            Duration::from_secs(20),
        )
        .await?;
        check_equal(
            terminal_classification(&startup)?,
            TerminalClassification::Ready,
            "signal fixture Host must start ready",
        )?;
        let router_process_id = wait_for_router_signal(&router_log, false).await?;
        let app_server_process_id = wait_for_process_id(&app_server_log).await?;
        let restart_socket = coordination_paths.operator_socket().to_owned();
        let restart_task = tokio::spawn(async move {
            send_operator_request(
                &restart_socket,
                OperatorRequest::RestartHost {
                    executable: invalid_executable,
                },
                Duration::from_secs(20),
            )
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))
        });
        let _signalled_router = wait_for_router_signal(&router_log, true).await?;
        signal_process(host_process_id, rustix::process::Signal::TERM)?;

        let host_status =
            tokio::time::timeout(Duration::from_secs(15), host_process.wait()).await??;
        check(
            host_status.success(),
            "signal-owned shutdown fixture Host must exit cleanly",
        )?;
        let restart_frames = tokio::time::timeout(Duration::from_secs(2), restart_task).await???;
        check(
            restart_frames
                .iter()
                .all(|frame| matches!(frame, OperatorFrame::Progress(_)))
                && restart_frames.iter().any(|frame| {
                    matches!(
                        frame,
                        OperatorFrame::Progress(
                            codex_router_host::HostProgress::ReplacementStarting
                        )
                    )
                })
                && !restart_frames.iter().any(|frame| {
                    matches!(
                        frame,
                        OperatorFrame::Progress(codex_router_host::HostProgress::ReExecuting)
                    )
                }),
            "signal-owned shutdown must close the restart exchange without finalizing replacement",
        )?;
        check(
            process_is_running(router_process_id),
            "signal settlement must retain the router that exceeded its shutdown bound",
        )?;
        check(
            !process_is_running(app_server_process_id),
            "signal settlement must preserve completed app-server teardown",
        )?;
        let reacquired = HostInstance::acquire(coordination_paths.clone())?;
        drop(reacquired);
        Ok(())
    }
    .await;

    let host_cleanup_result = reap_fixture_host(&mut host_process).await;
    let child_cleanup_result =
        terminate_logged_fixture_processes(&[router_log.as_path(), app_server_log.as_path()]);
    finish_fixture_proof(proof_result, [host_cleanup_result, child_cleanup_result])
}

#[tokio::test]
async fn host_replacement_signal_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_HOST_SIGNAL_CHILD").is_none() {
        return Ok(());
    }
    let current_executable = std::env::current_exe()?;
    let router_address =
        std::env::var("CODEX_HOST_SIGNAL_ROUTER_ADDRESS")?.parse::<SocketAddr>()?;
    let router_log = required_path("CODEX_HOST_SIGNAL_ROUTER_LOG")?;
    let app_server_socket = required_path("CODEX_HOST_SIGNAL_APP_SOCKET")?;
    let app_server_log = required_path("CODEX_HOST_SIGNAL_APP_LOG")?;
    let identity = codex_native_integration::executable_identity(&current_executable).await?;
    let app_server = AppServerLaunchPlan::new(
        ChildCommandSpec::new(current_executable.clone())
            .with_arguments([
                "--exact",
                "host_replacement_failure_app_server_entrypoint",
                "--nocapture",
            ])
            .with_environment("CODEX_HOST_FAILURE_APP_SOCKET", &app_server_socket)
            .with_environment("CODEX_HOST_FAILURE_APP_LOG", &app_server_log)
            .with_output(ChildOutput::Null),
        identity,
        "1.2.3".to_owned(),
    );
    let router_command = ChildCommandSpec::new(current_executable)
        .with_arguments([
            "--exact",
            "host_replacement_ignoring_router_entrypoint",
            "--nocapture",
        ])
        .with_environment(
            "CODEX_HOST_FAILURE_ROUTER_ADDRESS",
            router_address.to_string(),
        )
        .with_environment("CODEX_HOST_FAILURE_ROUTER_LOG", router_log)
        .with_output(ChildOutput::Null);
    let result = HostRuntime::run(
        HostConfig::new(HostConfigInputs {
            coordination_paths: HostCoordinationPaths::new(
                required_path("CODEX_HOST_SIGNAL_OPERATOR_SOCKET")?,
                required_path("CODEX_HOST_SIGNAL_INSTANCE_LOCK")?,
            ),
            router_endpoint: router_address,
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            app_server_socket,
            managed_executable: PathBuf::from("/unused/managed-codex"),
            deadlines: fixture_host_deadlines()?,
        }),
        ManagedChildLaunchPlans::new(Some(router_command), app_server),
        ManagedUpdateInputs::production().with_replacement_command(ChildCommandSpec::new(
            PathBuf::from("/configured/old/codex-router"),
        )),
    )
    .await;
    check_equal(
        result?,
        HostExit::Signal,
        "signal must own shutdown before replacement finalization is selected",
    )
}

#[tokio::test]
async fn host_replacement_ignoring_router_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let Some(address) = std::env::var_os("CODEX_HOST_FAILURE_ROUTER_ADDRESS") else {
        return Ok(());
    };
    let log = required_path("CODEX_HOST_FAILURE_ROUTER_LOG")?;
    run_ignoring_router_health_fixture(address.to_string_lossy().parse()?, &log).await
}

#[tokio::test]
async fn host_replacement_failure_app_server_entrypoint() -> Result<(), Box<dyn std::error::Error>>
{
    let Some(socket) = std::env::var_os("CODEX_HOST_FAILURE_APP_SOCKET") else {
        return Ok(());
    };
    let log = required_path("CODEX_HOST_FAILURE_APP_LOG")?;
    run_native_app_server_fixture(Path::new(&socket), "1.2.3", Some(&log)).await
}

async fn run_ignoring_router_health_fixture(
    address: SocketAddr,
    log: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    append_log(log, &format!("{}\n", std::process::id()))?;
    loop {
        tokio::select! {
            _ = terminate.recv() => append_log(log, "sigterm\n")?,
            accepted = listener.accept() => {
                let (mut stream, _peer) = accepted?;
                let mut request = [0_u8; 1024];
                let _read = stream.read(&mut request).await?;
                let body = serde_json::to_vec(&RouterCompatibility::current(false))?;
                let headers = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len(),
                );
                stream.write_all(headers.as_bytes()).await?;
                stream.write_all(&body).await?;
                stream.shutdown().await?;
            }
        }
    }
}

fn fixture_host_deadlines() -> Result<HostDeadlines, Box<dyn std::error::Error>> {
    Ok(HostDeadlines::new(HostDeadlineInputs {
        router_start: Duration::from_secs(2),
        app_server_start: Duration::from_secs(2),
        remote_control: Duration::from_secs(1),
        endpoint_inspection: Duration::from_millis(200),
        operator_request: Duration::from_secs(15),
    })?)
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

async fn wait_for_router_signal(
    log: &Path,
    require_signal: bool,
) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let contents = std::fs::read_to_string(log).unwrap_or_default();
            if let Some(process_id) = contents.lines().next().and_then(|line| line.parse().ok())
                && (!require_signal || contents.lines().any(|line| line == "sigterm"))
            {
                return process_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}

async fn wait_for_process_id(log: &Path) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(process_id) = std::fs::read_to_string(log)
                .ok()
                .and_then(|contents| contents.lines().next()?.parse().ok())
            {
                return process_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}

fn append_log(log: &Path, value: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).open(log)?;
    file.write_all(value.as_bytes())
}

fn required_path(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(PathBuf::from(std::env::var_os(name).ok_or_else(|| {
        std::io::Error::other(format!("{name} is missing"))
    })?))
}

fn process_is_running(process_id: u32) -> bool {
    i32::try_from(process_id)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|process_id| rustix::process::test_kill_process(process_id).is_ok())
}

fn signal_process(
    process_id: u32,
    signal: rustix::process::Signal,
) -> Result<(), Box<dyn std::error::Error>> {
    let process_id = rustix::process::Pid::from_raw(i32::try_from(process_id)?)
        .ok_or("fixture process ID must be nonzero")?;
    rustix::process::kill_process(process_id, signal)?;
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
    check(actual == expected, message)
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
            std::env::temp_dir().join(format!("crhr-{name}-{}-{counter}", std::process::id(),));
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
