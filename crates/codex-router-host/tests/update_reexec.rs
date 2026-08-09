use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
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
use codex_router_host::HostInstance;
use codex_router_host::HostRuntime;
use codex_router_host::ManagedChildLaunchPlans;
use codex_router_host::ManagedUpdateInputs;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::TerminalClassification;
use codex_router_host::UpdateDeadlines;
use codex_router_host::inherited_lock_environment;
use codex_router_test_support::native_app_server::run_native_app_server_fixture;
use codex_router_test_support::router_health::PersistentRouterHealthFixture;

#[path = "support/operator_client.rs"]
mod operator_client;
use operator_client::send_operator_request;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn update_matrix_uses_exact_managed_executable_and_preserves_children_before_activation()
-> Result<(), Box<dyn std::error::Error>> {
    run_update_case(
        UpdateFixtureMode::NoChange,
        TerminalClassification::Succeeded,
    )
    .await?;
    run_update_case(UpdateFixtureMode::Failure, TerminalClassification::Failed).await?;
    run_update_case(UpdateFixtureMode::Changed, TerminalClassification::Failed).await?;
    Ok(())
}

#[tokio::test]
async fn changed_update_tears_down_children_and_reexecs_with_continuous_lock()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("changed-reexec")?;
    let router = PersistentRouterHealthFixture::start().await?;
    let managed_executable = directory.path().join("managed-codex");
    install_updater_fixture(&managed_executable, UpdateFixtureMode::Changed)?;
    let coordination_paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let app_server_socket = directory.path().join("app.sock");
    let app_server_log = directory.path().join("app-pids.log");
    let replacement_marker = directory.path().join("replacement.log");
    std::fs::write(&app_server_log, b"")?;

    let mut host_process = tokio::process::Command::new(std::env::current_exe()?);
    host_process
        .args([
            "--exact",
            "changed_update_host_child_entrypoint",
            "--nocapture",
        ])
        .env("CODEX_HOST_UPDATE_CHILD", "1")
        .env(
            "CODEX_HOST_UPDATE_OPERATOR_SOCKET",
            coordination_paths.operator_socket(),
        )
        .env(
            "CODEX_HOST_UPDATE_INSTANCE_LOCK",
            coordination_paths.instance_lock(),
        )
        .env("CODEX_HOST_UPDATE_ROUTER", router.address().to_string())
        .env("CODEX_HOST_UPDATE_APP_SOCKET", &app_server_socket)
        .env("CODEX_HOST_UPDATE_APP_LOG", &app_server_log)
        .env("CODEX_HOST_UPDATE_MANAGED", &managed_executable)
        .env("CODEX_HOST_UPDATE_REPLACEMENT_MARKER", &replacement_marker)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let host_process = host_process.spawn()?;

    let frames = send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::UpdateCodex,
        Duration::from_secs(20),
    )
    .await?;
    check(
        matches!(frames.as_slice(), [OperatorFrame::Progress(_)]),
        "changed update must emit replacement-starting before old-host EOF",
    )?;
    let output =
        tokio::time::timeout(Duration::from_secs(20), host_process.wait_with_output()).await??;
    check(
        output.status.success(),
        &format!(
            "replacement bootstrap fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    check_equal(
        std::fs::read_to_string(&replacement_marker)?,
        "replacement-lock-valid\n".to_owned(),
        "replacement must validate and consume the continuously held lock",
    )?;
    let app_server_pid = wait_for_process_id(&app_server_log).await?;
    check(
        !process_is_running(app_server_pid),
        "changed update must stop the old app-server before exec",
    )?;
    let reacquired = HostInstance::acquire(coordination_paths)?;
    drop(reacquired);
    router.finish().await?;
    Ok(())
}

#[tokio::test]
async fn changed_update_host_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_HOST_UPDATE_CHILD").is_none() {
        return Ok(());
    }
    let operator_socket = required_path("CODEX_HOST_UPDATE_OPERATOR_SOCKET")?;
    let instance_lock = required_path("CODEX_HOST_UPDATE_INSTANCE_LOCK")?;
    let app_server_socket = required_path("CODEX_HOST_UPDATE_APP_SOCKET")?;
    let app_server_log = required_path("CODEX_HOST_UPDATE_APP_LOG")?;
    let managed_executable = required_path("CODEX_HOST_UPDATE_MANAGED")?;
    let router_endpoint = std::env::var("CODEX_HOST_UPDATE_ROUTER")?.parse()?;
    let current_executable = std::env::current_exe()?;
    let identity = codex_router_codex::executable_identity(&current_executable).await?;
    let app_server = AppServerLaunchPlan::new(
        ChildCommandSpec::new(current_executable.clone())
            .with_arguments([
                "--exact",
                "update_matrix_app_server_child_entrypoint",
                "--nocapture",
            ])
            .with_environment("CODEX_HOST_UPDATE_APP_SOCKET", &app_server_socket)
            .with_environment("CODEX_HOST_UPDATE_APP_LOG", &app_server_log)
            .with_output(ChildOutput::Null),
        identity,
        "1.2.3".to_owned(),
    );
    let replacement = ChildCommandSpec::new(current_executable).with_arguments([
        "--exact",
        "changed_update_replacement_child_entrypoint",
        "--nocapture",
    ]);
    let result = HostRuntime::run(
        HostConfig::new(HostConfigInputs {
            coordination_paths: HostCoordinationPaths::new(operator_socket, instance_lock),
            router_endpoint,
            app_server_socket,
            managed_executable,
            deadlines: fixture_host_deadlines()?,
        }),
        ManagedChildLaunchPlans::new(None, app_server),
        ManagedUpdateInputs::production()
            .with_deadlines(fixture_update_deadlines()?)
            .with_replacement_command(replacement),
    )
    .await;
    Err(format!("changed-update host returned unexpectedly: {result:?}").into())
}

#[tokio::test]
async fn changed_update_replacement_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let Some(marker) = std::env::var_os(inherited_lock_environment()) else {
        return Ok(());
    };
    let coordination_paths = HostCoordinationPaths::new(
        required_path("CODEX_HOST_UPDATE_OPERATOR_SOCKET")?,
        required_path("CODEX_HOST_UPDATE_INSTANCE_LOCK")?,
    );
    let owner = HostInstance::acquire_inherited(coordination_paths, &marker)?;
    std::fs::write(
        required_path("CODEX_HOST_UPDATE_REPLACEMENT_MARKER")?,
        b"replacement-lock-valid\n",
    )?;
    drop(owner);
    Ok(())
}

#[tokio::test]
async fn update_matrix_app_server_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let Some(socket_path) = std::env::var_os("CODEX_HOST_UPDATE_APP_SOCKET") else {
        return Ok(());
    };
    let process_log = std::env::var_os("CODEX_HOST_UPDATE_APP_LOG")
        .ok_or("update fixture app-server process log is missing")?;
    run_native_app_server_fixture(
        Path::new(&socket_path),
        "1.2.3",
        Some(Path::new(&process_log)),
    )
    .await
}

async fn run_update_case(
    mode: UpdateFixtureMode,
    expected: TerminalClassification,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new(mode.name())?;
    let router = PersistentRouterHealthFixture::start().await?;
    let managed_executable = directory.path().join("managed-codex");
    install_updater_fixture(&managed_executable, mode)?;
    let invocation_log = managed_executable.with_extension("log");
    let app_server_socket = directory.path().join("app.sock");
    let app_server_log = directory.path().join("app-pids.log");
    std::fs::write(&app_server_log, b"")?;
    let current_executable = std::env::current_exe()?;
    let identity = codex_router_codex::executable_identity(&current_executable).await?;
    let app_server = AppServerLaunchPlan::new(
        ChildCommandSpec::new(current_executable)
            .with_arguments([
                "--exact",
                "update_matrix_app_server_child_entrypoint",
                "--nocapture",
            ])
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
            app_server_socket,
            managed_executable: managed_executable.clone(),
            deadlines: HostDeadlines::new(HostDeadlineInputs {
                router_start: Duration::from_secs(1),
                app_server_start: Duration::from_secs(2),
                remote_control: Duration::from_secs(1),
                endpoint_inspection: Duration::from_millis(200),
                operator_request: Duration::from_secs(15),
            })?,
        }),
        ManagedChildLaunchPlans::new(None, app_server),
        ManagedUpdateInputs::production().with_deadlines(UpdateDeadlines::new(
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
    check_equal(
        std::fs::read_to_string(&invocation_log)?,
        format!(
            "{} update\n",
            std::fs::canonicalize(&managed_executable)?.display()
        ),
        "updater must invoke the exact captured executable with update",
    )?;
    check(
        process_is_running(app_server_pid),
        "pre-activation update result must preserve the running app-server",
    )?;

    runtime.abort();
    let _runtime_result = runtime.await;
    kill_process(app_server_pid)?;
    router.finish().await?;
    Ok(())
}

fn fixture_host_deadlines() -> Result<HostDeadlines, Box<dyn std::error::Error>> {
    Ok(HostDeadlines::new(HostDeadlineInputs {
        router_start: Duration::from_secs(1),
        app_server_start: Duration::from_secs(2),
        remote_control: Duration::from_secs(1),
        endpoint_inspection: Duration::from_millis(200),
        operator_request: Duration::from_secs(15),
    })?)
}

fn fixture_update_deadlines() -> Result<UpdateDeadlines, Box<dyn std::error::Error>> {
    Ok(UpdateDeadlines::new(
        Duration::from_secs(4),
        Duration::from_secs(15),
        Duration::from_millis(200),
        Duration::from_millis(200),
    )?)
}

fn required_path(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let value = std::env::var_os(name)
        .ok_or_else(|| std::io::Error::other(format!("{name} is missing")))?;
    Ok(PathBuf::from(value))
}

#[derive(Clone, Copy)]
enum UpdateFixtureMode {
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

fn install_updater_fixture(
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
        format!("#!/bin/sh\nprintf '%s %s\\n' \"$0\" \"$1\" > \"$0.log\"\n{action}\n"),
    )?;
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700))?;
    if matches!(mode, UpdateFixtureMode::Changed) {
        std::fs::write(
            executable.with_extension("replacement"),
            b"#!/bin/sh\nexit 0\nchanged-content\n",
        )?;
    }
    Ok(())
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
        .ok_or_else(|| std::io::Error::other("update terminal response is missing").into())
}

async fn wait_for_process_id(process_log: &Path) -> Result<u32, Box<dyn std::error::Error>> {
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

fn process_is_running(process_id: u32) -> bool {
    i32::try_from(process_id)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|process_id| rustix::process::test_kill_process(process_id).is_ok())
}

fn kill_process(process_id: u32) -> Result<(), Box<dyn std::error::Error>> {
    let process_id = rustix::process::Pid::from_raw(i32::try_from(process_id)?)
        .ok_or("fixture process ID must be nonzero")?;
    rustix::process::kill_process(process_id, rustix::process::Signal::KILL)?;
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
            std::env::temp_dir().join(format!("crhu-{name}-{}-{counter}", std::process::id()));
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
