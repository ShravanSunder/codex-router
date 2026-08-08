use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_router_codex::executable_identity;
use codex_router_host::APP_SERVER_FORCE_AFTER;
use codex_router_host::APP_SERVER_SHUTDOWN_TOTAL;
use codex_router_host::AppServerChild;
use codex_router_host::AppServerEndpointError;
use codex_router_host::AppServerReadiness;
use codex_router_host::AppServerShutdownDeadlines;
use codex_router_host::ExpectedExit;
use codex_router_host::ProcessGroupChild;
use codex_router_host::ROUTER_SHUTDOWN_TIMEOUT;
use codex_router_host::RouterChild;
use codex_router_host::RouterOwnership;
use codex_router_host::RouterProbeResult;
use codex_router_host::RouterShutdownOutcome;
use codex_router_host::ShutdownAction;
use codex_router_host::ShutdownOutcome;
use codex_router_host::probe_router;
use codex_router_host::require_unowned_app_server_endpoint;
use codex_router_test_support::native_app_server::run_delayed_native_app_server_observation_fixture;
use codex_router_test_support::native_app_server::run_native_app_server_fixture;
use codex_router_test_support::router_health::RouterHealthFixture;
use codex_router_test_support::router_health::RouterHealthFixtureResponse;
use codex_router_test_support::signal_recording::SignalFixtureMode;
use codex_router_test_support::signal_recording::run_signal_fixture;
use tokio::process::Command;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn app_server_shutdown_policy_uses_the_pinned_60_and_70_second_boundaries() {
    assert_eq!(APP_SERVER_FORCE_AFTER, Duration::from_secs(60));
    assert_eq!(APP_SERVER_SHUTDOWN_TOTAL, Duration::from_secs(70));

    let mut expected_exit = ExpectedExit::new(4100);
    assert_eq!(
        expected_exit.next_action(Duration::ZERO, true),
        ShutdownAction::SendTerminate
    );
    assert_eq!(
        expected_exit.next_action(Duration::from_secs(59), true),
        ShutdownAction::Wait
    );
    assert_eq!(
        expected_exit.next_action(Duration::from_secs(60), true),
        ShutdownAction::SendKill
    );
    assert_eq!(
        expected_exit.next_action(Duration::from_secs(69), true),
        ShutdownAction::Wait
    );
    assert_eq!(
        expected_exit.next_action(Duration::from_secs(70), true),
        ShutdownAction::TimedOutStillRunning
    );
    assert_eq!(
        expected_exit.next_action(Duration::from_secs(71), false),
        ShutdownAction::Complete(ShutdownOutcome::Forced)
    );
    assert!(expected_exit.term_sent());
    assert!(expected_exit.kill_sent());
}

#[tokio::test]
async fn owned_app_server_is_isolated_and_receives_one_exact_pid_sigterm()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("graceful")?;
    let event_file = directory.path().join("events.log");
    let identity = executable_identity(&std::env::current_exe()?).await?;
    let mut command = signal_fixture_command(SignalFixtureMode::ExitOnTerminate, &event_file)?;
    let process = ProcessGroupChild::spawn(&mut command)?;
    wait_for_fixture_ready(&event_file).await?;
    let process_id = process.process_id();
    let process_group_id = rustix::process::getpgid(Some(
        rustix::process::Pid::from_raw(i32::try_from(process_id)?)
            .ok_or("child process id must be nonzero")?,
    ))?;
    check_equal(
        process_group_id.as_raw_nonzero().get(),
        i32::try_from(process_id)?,
        "owned app-server PID must lead its isolated process group",
    )?;

    let mut app_server = AppServerChild::new(process, identity);
    let outcome = app_server.shutdown().await?;

    check_equal(
        outcome,
        ShutdownOutcome::Graceful,
        "SIGTERM-aware app-server must stop gracefully",
    )?;
    check_equal(
        std::fs::read_to_string(&event_file)?,
        "ready\nsigterm\n".to_owned(),
        "graceful path must send exactly one SIGTERM",
    )?;
    check_equal(
        app_server.expected_exit().map(ExpectedExit::child_id),
        Some(process_id),
        "expected-exit token must name the exact child",
    )?;
    check_equal(
        app_server.expected_exit().map(ExpectedExit::term_sent),
        Some(true),
        "expected-exit token must retain SIGTERM progress",
    )?;
    check_equal(
        app_server.expected_exit().map(ExpectedExit::kill_sent),
        Some(false),
        "graceful path must not record SIGKILL",
    )?;
    Ok(())
}

#[tokio::test]
async fn app_server_force_escalation_signals_and_reaps_once()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("forced")?;
    let event_file = directory.path().join("events.log");
    let identity = executable_identity(&std::env::current_exe()?).await?;
    let mut command = signal_fixture_command(SignalFixtureMode::IgnoreTerminate, &event_file)?;
    let process = ProcessGroupChild::spawn(&mut command)?;
    wait_for_fixture_ready(&event_file).await?;
    let mut app_server = AppServerChild::new(process, identity);

    let fixture_deadlines =
        AppServerShutdownDeadlines::new(Duration::from_millis(50), Duration::from_secs(2))
            .ok_or("fixture shutdown deadlines must be ordered")?;
    check_equal(
        app_server
            .shutdown_with_deadlines(fixture_deadlines)
            .await?,
        ShutdownOutcome::Forced,
        "SIGTERM-ignoring app-server must use the pinned force escalation",
    )?;
    check_equal(
        app_server.expected_exit().map(ExpectedExit::term_sent),
        Some(true),
        "force path must retain the first-signal progress",
    )?;
    check_equal(
        app_server.expected_exit().map(ExpectedExit::kill_sent),
        Some(true),
        "force path must retain the one SIGKILL escalation",
    )?;
    check_equal(
        std::fs::read_to_string(&event_file)?,
        "ready\nsigterm\n".to_owned(),
        "fixture must observe exactly one catchable signal before SIGKILL",
    )?;
    Ok(())
}

#[tokio::test]
async fn router_probe_accepts_only_compatible_tokenless_health()
-> Result<(), Box<dyn std::error::Error>> {
    let compatible = RouterHealthFixture::start(RouterHealthFixtureResponse::Compatible).await?;
    check_equal(
        probe_router(compatible.address(), Duration::from_secs(2)).await?,
        RouterProbeResult::Compatible,
        "exact compatible router must be accepted",
    )?;
    compatible.finish().await?;

    let authentication_required =
        RouterHealthFixture::start(RouterHealthFixtureResponse::AuthenticationRequired).await?;
    check_equal(
        probe_router(authentication_required.address(), Duration::from_secs(2)).await?,
        RouterProbeResult::AuthenticationRequired,
        "auth-required router must fail host compatibility",
    )?;
    authentication_required.finish().await?;

    let incompatible =
        RouterHealthFixture::start(RouterHealthFixtureResponse::Incompatible).await?;
    check_equal(
        probe_router(incompatible.address(), Duration::from_secs(2)).await?,
        RouterProbeResult::Incompatible,
        "foreign router identity must fail closed",
    )?;
    incompatible.finish().await?;

    let squatter = RouterHealthFixture::start(RouterHealthFixtureResponse::Malformed).await?;
    check_equal(
        probe_router(squatter.address(), Duration::from_secs(2)).await?,
        RouterProbeResult::Incompatible,
        "socket squatter must fail closed",
    )?;
    squatter.finish().await?;

    let unavailable_listener =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let unavailable_address = unavailable_listener.local_addr()?;
    drop(unavailable_listener);
    check_equal(
        probe_router(unavailable_address, Duration::from_secs(2)).await?,
        RouterProbeResult::Unavailable,
        "absent listener must remain distinct from incompatibility",
    )?;
    Ok(())
}

#[tokio::test]
async fn app_server_owner_rejects_foreign_endpoint_and_observes_native_readiness()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("native-app-server")?;
    let squatter_socket = directory.path().join("squatter.sock");
    let squatter = tokio::net::UnixListener::bind(&squatter_socket)?;
    check(
        matches!(
            require_unowned_app_server_endpoint(&squatter_socket, Duration::from_millis(200)).await,
            Err(AppServerEndpointError::OwnershipConflict)
        ),
        "reachable foreign app-server endpoint must fail before spawn",
    )?;
    drop(squatter);

    let socket_path = directory.path().join("app-server.sock");
    require_unowned_app_server_endpoint(&socket_path, Duration::from_millis(200)).await?;
    let identity = executable_identity(&std::env::current_exe()?).await?;
    let mut command = native_app_server_fixture_command(&socket_path, "1.2.3")?;
    let mut app_server = AppServerChild::spawn(&mut command, identity, "1.2.3".to_owned())?;

    let readiness = app_server
        .await_readiness(&socket_path, Duration::from_secs(2), Duration::from_secs(1))
        .await?;
    check_equal(
        readiness,
        AppServerReadiness::Ready {
            running_version: "1.2.3".to_owned(),
        },
        "native initialize and connected Remote Control must be ready",
    )?;
    check_equal(
        app_server.shutdown().await?,
        ShutdownOutcome::Graceful,
        "fixture app-server must stop through the shared SIGTERM path",
    )?;
    check(
        !socket_path.exists(),
        "fixture app-server must release the conventional socket",
    )?;
    Ok(())
}

#[tokio::test]
async fn app_server_readiness_gives_remote_control_its_own_deadline_after_native_ready()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("composed-readiness")?;
    let socket_path = directory.path().join("app-server.sock");
    let fixture_socket = socket_path.clone();
    let server = tokio::spawn(async move {
        run_delayed_native_app_server_observation_fixture(
            &fixture_socket,
            "1.2.3",
            Duration::from_millis(80),
            Duration::from_millis(80),
        )
        .await
    });

    let event_file = directory.path().join("events.log");
    let identity = executable_identity(&std::env::current_exe()?).await?;
    let mut command = signal_fixture_command(SignalFixtureMode::ExitOnTerminate, &event_file)?;
    let mut app_server = AppServerChild::spawn(&mut command, identity, "1.2.3".to_owned())?;
    let readiness = app_server
        .await_readiness(
            &socket_path,
            Duration::from_millis(120),
            Duration::from_millis(120),
        )
        .await?;
    check_equal(
        readiness,
        AppServerReadiness::Ready {
            running_version: "1.2.3".to_owned(),
        },
        "native and Remote Control convergence must receive separate deadlines",
    )?;
    check_equal(
        app_server.shutdown().await?,
        ShutdownOutcome::Graceful,
        "readiness fixture child must stop gracefully",
    )?;
    server.await?.map_err(std::io::Error::other)?;
    Ok(())
}

#[tokio::test]
async fn owned_router_uses_sigterm_only_and_external_router_has_no_child_handle()
-> Result<(), Box<dyn std::error::Error>> {
    check_equal(
        ROUTER_SHUTDOWN_TIMEOUT,
        Duration::from_secs(10),
        "router stop boundary must remain ten seconds",
    )?;
    check_equal(
        RouterOwnership::External,
        RouterOwnership::External,
        "external ownership must remain handle-free",
    )?;

    let directory = TestDirectory::new("router-stop")?;
    let event_file = directory.path().join("events.log");
    let mut command = signal_fixture_command(SignalFixtureMode::ExitOnTerminate, &event_file)?;
    let mut router = RouterChild::spawn(&mut command)?;
    wait_for_fixture_ready(&event_file).await?;

    check_equal(
        router.shutdown().await?,
        RouterShutdownOutcome::Graceful,
        "owned router must exit after its exact SIGTERM",
    )?;
    check_equal(
        std::fs::read_to_string(&event_file)?,
        "ready\nsigterm\n".to_owned(),
        "owned router must receive exactly one SIGTERM and no SIGKILL",
    )?;
    Ok(())
}

#[tokio::test]
async fn updater_group_signal_terminates_parent_and_descendant()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("updater-process-group")?;
    let process_log = directory.path().join("processes.log");
    std::fs::write(&process_log, b"")?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--exact")
        .arg("updater_group_parent_child_entrypoint")
        .arg("--nocapture")
        .env("CODEX_ROUTER_HOST_UPDATER_GROUP_PARENT", "1")
        .env("CODEX_ROUTER_HOST_UPDATER_GROUP_LOG", &process_log)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut updater = ProcessGroupChild::spawn(&mut command)?;
    let process_ids = wait_for_process_ids(&process_log, 2).await?;

    updater.send_group_terminate()?;
    let _status = tokio::time::timeout(Duration::from_secs(2), updater.wait()).await??;
    wait_for_process_exit(process_ids[1]).await?;
    Ok(())
}

#[tokio::test]
async fn updater_group_parent_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_UPDATER_GROUP_PARENT").is_none() {
        return Ok(());
    }
    let process_log = std::env::var_os("CODEX_ROUTER_HOST_UPDATER_GROUP_LOG")
        .ok_or("updater group process log is missing")?;
    append_process_id(Path::new(&process_log))?;
    let mut descendant = Command::new(std::env::current_exe()?);
    descendant
        .arg("--exact")
        .arg("updater_group_descendant_child_entrypoint")
        .arg("--nocapture")
        .env("CODEX_ROUTER_HOST_UPDATER_GROUP_DESCENDANT", "1")
        .env("CODEX_ROUTER_HOST_UPDATER_GROUP_LOG", &process_log)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _descendant = descendant.spawn()?;
    std::future::pending::<()>().await;
    Ok(())
}

#[tokio::test]
async fn updater_group_descendant_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_UPDATER_GROUP_DESCENDANT").is_none() {
        return Ok(());
    }
    let process_log = std::env::var_os("CODEX_ROUTER_HOST_UPDATER_GROUP_LOG")
        .ok_or("updater group process log is missing")?;
    append_process_id(Path::new(&process_log))?;
    std::future::pending::<()>().await;
    Ok(())
}

fn append_process_id(process_log: &Path) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new().append(true).open(process_log)?;
    writeln!(file, "{}", std::process::id())
}

async fn wait_for_process_ids(
    process_log: &Path,
    expected_count: usize,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(2), async {
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

async fn wait_for_process_exit(process_id: u32) -> Result<(), Box<dyn std::error::Error>> {
    let process_id = rustix::process::Pid::from_raw(i32::try_from(process_id)?)
        .ok_or("fixture process ID must be nonzero")?;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if rustix::process::test_kill_process(process_id).is_err() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn signal_recording_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let Some(mode) = std::env::var_os("CODEX_ROUTER_HOST_SIGNAL_FIXTURE_MODE") else {
        return Ok(());
    };
    let event_file = std::env::var_os("CODEX_ROUTER_HOST_SIGNAL_EVENT_FILE")
        .ok_or("signal fixture event file is missing")?;
    let mode = match mode.to_str() {
        Some("exit-on-terminate") => SignalFixtureMode::ExitOnTerminate,
        Some("ignore-terminate") => SignalFixtureMode::IgnoreTerminate,
        _ => return Err("unknown signal fixture mode".into()),
    };
    run_signal_fixture(mode, Path::new(&event_file)).await?;
    Ok(())
}

#[tokio::test]
async fn native_app_server_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let Some(socket_path) = std::env::var_os("CODEX_ROUTER_HOST_NATIVE_SOCKET") else {
        return Ok(());
    };
    let version = std::env::var("CODEX_ROUTER_HOST_NATIVE_VERSION")?;
    run_native_app_server_fixture(Path::new(&socket_path), &version, None).await
}

fn signal_fixture_command(
    mode: SignalFixtureMode,
    event_file: &Path,
) -> Result<Command, Box<dyn std::error::Error>> {
    let mode = match mode {
        SignalFixtureMode::ExitOnTerminate => "exit-on-terminate",
        SignalFixtureMode::IgnoreTerminate => "ignore-terminate",
    };
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--exact")
        .arg("signal_recording_child_entrypoint")
        .arg("--nocapture")
        .env("CODEX_ROUTER_HOST_SIGNAL_FIXTURE_MODE", mode)
        .env("CODEX_ROUTER_HOST_SIGNAL_EVENT_FILE", event_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

fn native_app_server_fixture_command(
    socket_path: &Path,
    running_version: &str,
) -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--exact")
        .arg("native_app_server_child_entrypoint")
        .arg("--nocapture")
        .env("CODEX_ROUTER_HOST_NATIVE_SOCKET", socket_path)
        .env("CODEX_ROUTER_HOST_NATIVE_VERSION", running_version)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

async fn wait_for_fixture_ready(event_file: &Path) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                std::fs::read_to_string(event_file).as_deref(),
                Ok("ready\n")
            ) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
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
            std::env::temp_dir().join(format!("crh-{name}-{}-{counter}", std::process::id()));
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
