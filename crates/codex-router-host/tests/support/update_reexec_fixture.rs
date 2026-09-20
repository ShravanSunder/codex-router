use std::os::unix::fs::PermissionsExt;
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
use codex_router_host::HostRuntime;
use codex_router_host::ManagedChildLaunchPlans;
use codex_router_host::ManagedUpdateInputs;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::TerminalClassification;
use codex_router_host::UpdateDeadlines;
use codex_router_test_support::router_health::PersistentRouterHealthFixture;

use super::operator_client::send_operator_request;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn assert_explicit_restart_receipt(
    receipt_path: &Path,
    expected_process_id: u32,
    expected_executable: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let receipt = std::fs::read_to_string(receipt_path)?;
    let mut receipt_lines = receipt.lines();
    let expected_process_id = expected_process_id.to_string();
    let expected_executable = expected_executable.to_string_lossy();
    check_equal(
        receipt_lines.next(),
        Some(expected_process_id.as_str()),
        "same-process exec must preserve the Host PID",
    )?;
    check_equal(
        receipt_lines.next(),
        Some(expected_executable.as_ref()),
        "restart must execute the executable selected by the requesting CLI",
    )?;
    check_equal(
        receipt_lines.next(),
        Some("retained-host-environment"),
        "restart must retain Host-owned replacement environment",
    )
}

pub(super) async fn run_update_case(
    mode: UpdateFixtureMode,
    expected: TerminalClassification,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new(mode.name())?;
    let router = PersistentRouterHealthFixture::start().await?;
    let managed_executable = directory.path().join("managed-codex");
    install_updater_fixture(&managed_executable, mode)?;
    let updater_executable = std::fs::canonicalize(&managed_executable)?;
    let invocation_log = managed_executable.with_extension("log");
    let app_server_socket = directory.path().join("app.sock");
    let app_server_log = directory.path().join("app-pids.log");
    std::fs::write(&app_server_log, b"")?;
    let current_executable = std::env::current_exe()?;
    let identity = codex_native_integration::executable_identity(&managed_executable).await?;
    let app_server = AppServerLaunchPlan::new(
        ChildCommandSpec::new(managed_executable.clone())
            .with_environment("CODEX_HOST_UPDATE_TEST_BINARY", &current_executable)
            .with_environment("CODEX_HOST_UPDATE_APP_SOCKET", &app_server_socket)
            .with_environment("CODEX_HOST_UPDATE_APP_LOG", &app_server_log)
            .with_output(ChildOutput::Null),
        identity,
        "1.2.3".to_owned(),
    );
    let coordination_paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let runtime = tokio::spawn(HostRuntime::run(
        HostConfig::new(HostConfigInputs {
            coordination_paths: coordination_paths.clone(),
            router_endpoint: router.address(),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            app_server_socket,
            managed_executable: managed_executable.clone(),
            deadlines: HostDeadlines::new(HostDeadlineInputs {
                router_start: Duration::from_secs(1),
                app_server_start: Duration::from_secs(5),
                remote_control: Duration::from_secs(1),
                endpoint_inspection: Duration::from_millis(200),
                operator_request: Duration::from_secs(15),
            })?,
        }),
        ManagedChildLaunchPlans::new(None, app_server),
        ManagedUpdateInputs::production()
            .with_updater_command(
                ChildCommandSpec::new(updater_executable).with_arguments(["update"]),
            )
            .with_deadlines(UpdateDeadlines::new(
                Duration::from_secs(4),
                Duration::from_secs(15),
                Duration::from_millis(200),
                Duration::from_millis(200),
            )?),
    ));

    let startup = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::AwaitHostStart,
        Duration::from_secs(20),
    )
    .await?;
    check_equal(
        terminal_classification(&startup)?,
        TerminalClassification::Ready,
        "update fixture host must start ready",
    )?;
    let app_server_pid = wait_for_process_id(&app_server_log).await?;
    let update = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::UpdateCodex,
        Duration::from_secs(20),
    )
    .await?;
    check_equal(
        terminal_classification(&update)?,
        expected,
        "update classification must match fixture behavior",
    )?;
    let invocation = std::fs::read_to_string(&invocation_log)?;
    check_equal(
        invocation,
        format!(
            "{} update\n",
            std::fs::canonicalize(&managed_executable)?.display()
        ),
        "updater must invoke the exact captured executable with update",
    )?;
    let app_server_pids = std::fs::read_to_string(&app_server_log)?
        .lines()
        .filter_map(|line| line.parse::<u32>().ok())
        .collect::<Vec<_>>();
    if matches!(mode, UpdateFixtureMode::Changed) {
        check(
            app_server_pids.len() >= 2,
            "changed update must spawn a replacement app-server",
        )?;
        check(
            !process_is_running(app_server_pid),
            "changed update must stop the old app-server",
        )?;
        check(
            process_is_running(
                *app_server_pids
                    .last()
                    .ok_or("replacement app-server PID is missing")?,
            ),
            "replacement app-server must remain running",
        )?;
    } else {
        check(
            process_is_running(app_server_pid),
            "failed/no-change update must preserve the running app-server",
        )?;
    }

    runtime.abort();
    let _runtime_result = runtime.await;
    for process_id in app_server_pids {
        if process_is_running(process_id) {
            kill_process(process_id)?;
        }
    }
    router.finish().await?;
    Ok(())
}

pub(super) async fn run_update_with_blocked_installer_case()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("blocked-installer")?;
    let router = PersistentRouterHealthFixture::start().await?;
    let managed_executable = directory.path().join("managed-codex");
    install_updater_fixture(&managed_executable, UpdateFixtureMode::Changed)?;
    let replacement_executable = managed_executable.with_extension("replacement");
    let installer_started = directory.path().join("installer-started");
    let installer_release = directory.path().join("installer-release");
    let app_server_socket = directory.path().join("app.sock");
    let app_server_log = directory.path().join("app-pids.log");
    std::fs::write(&app_server_log, b"")?;
    let current_executable = std::env::current_exe()?;
    let identity = codex_native_integration::executable_identity(&managed_executable).await?;
    let app_server = AppServerLaunchPlan::new(
        ChildCommandSpec::new(managed_executable.clone())
            .with_environment("CODEX_HOST_UPDATE_TEST_BINARY", &current_executable)
            .with_environment("CODEX_HOST_UPDATE_APP_SOCKET", &app_server_socket)
            .with_environment("CODEX_HOST_UPDATE_APP_LOG", &app_server_log)
            .with_output(ChildOutput::Null),
        identity,
        "1.2.3".to_owned(),
    );
    let coordination_paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let updater_command = ChildCommandSpec::new(current_executable)
        .with_arguments([
            "--exact",
            "changed_update_installer_barrier_entrypoint",
            "--nocapture",
        ])
        .with_environment("CODEX_HOST_UPDATE_INSTALLER_STARTED", &installer_started)
        .with_environment("CODEX_HOST_UPDATE_INSTALLER_RELEASE", &installer_release)
        .with_environment("CODEX_HOST_UPDATE_MANAGED", &managed_executable)
        .with_environment("CODEX_HOST_UPDATE_REPLACEMENT", &replacement_executable);
    let runtime = tokio::spawn(HostRuntime::run(
        HostConfig::new(HostConfigInputs {
            coordination_paths: coordination_paths.clone(),
            router_endpoint: router.address(),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            app_server_socket: app_server_socket.clone(),
            managed_executable,
            deadlines: fixture_host_deadlines()?,
        }),
        ManagedChildLaunchPlans::new(None, app_server),
        ManagedUpdateInputs::production()
            .with_updater_command(updater_command)
            .with_deadlines(fixture_update_deadlines()?),
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
            "blocked-installer fixture Host must start ready",
        )?;
        let original_app_server_pid = wait_for_process_id(&app_server_log).await?;
        let update_socket = coordination_paths.operator_socket().to_path_buf();
        let update_task = tokio::spawn(async move {
            send_operator_request(
                &update_socket,
                OperatorRequest::UpdateCodex,
                Duration::from_secs(20),
            )
            .await
            .map_err(|error| error.to_string())
        });
        wait_for_path(&installer_started).await?;

        let status = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::Status,
            Duration::from_secs(5),
        )
        .await?;
        check_equal(
            terminal_classification(&status)?,
            TerminalClassification::Ready,
            "operator socket must remain ready while the installer is blocked",
        )?;
        check(
            process_is_running(original_app_server_pid),
            "original app-server must remain running while the installer is blocked",
        )?;
        let _serving_connection = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::net::UnixStream::connect(&app_server_socket),
        )
        .await??;

        std::fs::write(&installer_release, b"release\n")?;
        let update = update_task.await??;
        check_equal(
            terminal_classification(&update)?,
            TerminalClassification::Succeeded,
            "released changed update must succeed",
        )?;
        let app_server_pids = wait_for_process_ids(&app_server_log, 2).await?;
        check(
            !process_is_running(original_app_server_pid),
            "changed update must stop the original app-server after installation",
        )?;
        check(
            process_is_running(
                *app_server_pids
                    .last()
                    .ok_or("replacement app-server PID is missing")?,
            ),
            "changed update must leave the replacement app-server running",
        )?;
        let final_status = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::Status,
            Duration::from_secs(5),
        )
        .await?;
        check_equal(
            terminal_classification(&final_status)?,
            TerminalClassification::Ready,
            "Host, router, and operator socket must remain ready after app-server update",
        )
    }
    .await;

    let _release_result = std::fs::write(&installer_release, b"release\n");
    runtime.abort();
    let _runtime_result = runtime.await;
    for process_id in read_process_ids(&app_server_log) {
        if process_is_running(process_id) {
            kill_process(process_id)?;
        }
    }
    let router_cleanup_result = router.finish().await;
    proof_result?;
    router_cleanup_result
}

pub(super) fn fixture_host_deadlines() -> Result<HostDeadlines, Box<dyn std::error::Error>> {
    Ok(HostDeadlines::new(HostDeadlineInputs {
        router_start: Duration::from_secs(1),
        app_server_start: Duration::from_secs(5),
        remote_control: Duration::from_secs(1),
        endpoint_inspection: Duration::from_millis(200),
        operator_request: Duration::from_secs(15),
    })?)
}

pub(super) fn fixture_update_deadlines() -> Result<UpdateDeadlines, Box<dyn std::error::Error>> {
    Ok(UpdateDeadlines::new(
        Duration::from_secs(4),
        Duration::from_secs(15),
        Duration::from_millis(200),
        Duration::from_millis(200),
    )?)
}

pub(super) fn required_path(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let value = std::env::var_os(name)
        .ok_or_else(|| std::io::Error::other(format!("{name} is missing")))?;
    Ok(PathBuf::from(value))
}

#[derive(Clone, Copy)]
pub(super) enum UpdateFixtureMode {
    NoChange,
    Failure,
    Changed,
}

impl UpdateFixtureMode {
    const fn name(self) -> &'static str {
        match self {
            Self::NoChange => "no-change",
            Self::Failure => "failure",
            Self::Changed => "changed",
        }
    }
}

pub(super) fn install_updater_fixture(
    executable: &Path,
    mode: UpdateFixtureMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let action = match mode {
        UpdateFixtureMode::NoChange => "exit 0",
        UpdateFixtureMode::Failure => "exit 9",
        UpdateFixtureMode::Changed => "cp \"$0.replacement\" \"$0\"; chmod 700 \"$0\"; exit 0",
    };
    std::fs::write(
        executable,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nprintf '%s %s\\n' \"$0\" \"$1\" > \"$0.log\"\nif [ \"$1\" = \"update\" ]; then {action}; fi\nexec \"$CODEX_HOST_UPDATE_TEST_BINARY\" --exact update_matrix_app_server_child_entrypoint --nocapture\n"
        ),
    )?;
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700))?;
    if matches!(mode, UpdateFixtureMode::Changed) {
        std::fs::write(
            executable.with_extension("replacement"),
            b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nexec \"$CODEX_HOST_UPDATE_TEST_BINARY\" --exact update_matrix_app_server_child_entrypoint --nocapture\nchanged-content\n",
        )?;
    }
    Ok(())
}

pub(super) fn terminal_classification(
    frames: &[OperatorFrame],
) -> Result<TerminalClassification, Box<dyn std::error::Error>> {
    frames
        .iter()
        .find_map(|frame| match frame {
            OperatorFrame::Terminal(response) => Some(response.classification()),
            OperatorFrame::Progress(_) => None,
        })
        .ok_or_else(|| std::io::Error::other("update terminal response is missing").into())
}

pub(super) async fn wait_for_process_id(
    process_log: &Path,
) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(process_id) = std::fs::read_to_string(process_log)
                .ok()
                .and_then(|contents| contents.lines().next()?.parse::<u32>().ok())
            {
                return process_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}

async fn wait_for_process_ids(
    process_log: &Path,
    expected_count: usize,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let process_ids = read_process_ids(process_log);
            if process_ids.len() >= expected_count {
                return process_ids;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}

async fn wait_for_path(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(15), async {
        while !path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await?)
}

fn read_process_ids(process_log: &Path) -> Vec<u32> {
    std::fs::read_to_string(process_log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.parse::<u32>().ok())
        .collect()
}

pub(super) fn process_is_running(process_id: u32) -> bool {
    i32::try_from(process_id)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|process_id| rustix::process::test_kill_process(process_id).is_ok())
}

pub(super) fn kill_process(process_id: u32) -> Result<(), Box<dyn std::error::Error>> {
    let process_id = rustix::process::Pid::from_raw(i32::try_from(process_id)?)
        .ok_or("fixture process ID must be nonzero")?;
    rustix::process::kill_process(process_id, rustix::process::Signal::KILL)?;
    Ok(())
}

pub(super) fn check_equal<TValue>(
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

pub(super) fn check(condition: bool, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

pub(super) struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    pub(super) fn new(name: &str) -> std::io::Result<Self> {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("crhu-{name}-{}-{counter}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _cleanup_result = std::fs::remove_dir_all(&self.path);
    }
}
