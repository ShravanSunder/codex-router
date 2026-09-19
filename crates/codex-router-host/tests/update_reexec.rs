use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use codex_router_host::AppServerLaunchPlan;
use codex_router_host::ChildCommandSpec;
use codex_router_host::ChildOutput;
use codex_router_host::HostConfig;
use codex_router_host::HostConfigInputs;
use codex_router_host::HostCoordinationPaths;
use codex_router_host::HostInstance;
use codex_router_host::HostRuntime;
use codex_router_host::ManagedChildLaunchPlans;
use codex_router_host::ManagedUpdateInputs;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::TerminalClassification;
use codex_router_host::inherited_lock_environment;
use codex_router_test_support::native_app_server::run_native_app_server_fixture;
use codex_router_test_support::router_health::PersistentRouterHealthFixture;

#[path = "support/operator_client.rs"]
mod operator_client;
use operator_client::send_operator_request;

#[path = "support/host_replacement_cleanup.rs"]
mod host_replacement_cleanup;
use host_replacement_cleanup::finish_fixture_proof;
use host_replacement_cleanup::reap_fixture_host;
use host_replacement_cleanup::terminate_logged_fixture_processes;

#[path = "support/update_reexec_fixture.rs"]
mod update_reexec_fixture;
use update_reexec_fixture::TestDirectory;
use update_reexec_fixture::UpdateFixtureMode;
use update_reexec_fixture::assert_explicit_restart_receipt;
use update_reexec_fixture::check;
use update_reexec_fixture::check_equal;
use update_reexec_fixture::fixture_host_deadlines;
use update_reexec_fixture::fixture_update_deadlines;
use update_reexec_fixture::install_updater_fixture;
use update_reexec_fixture::process_is_running;
use update_reexec_fixture::required_path;
use update_reexec_fixture::run_update_case;
use update_reexec_fixture::terminal_classification;
use update_reexec_fixture::wait_for_process_id;

#[tokio::test]
async fn update_matrix_uses_exact_managed_executable_and_preserves_children_before_activation()
-> Result<(), Box<dyn std::error::Error>> {
    run_update_case(
        UpdateFixtureMode::NoChange,
        TerminalClassification::Succeeded,
    )
    .await?;
    run_update_case(UpdateFixtureMode::Failure, TerminalClassification::Failed).await?;
    run_update_case(
        UpdateFixtureMode::Changed,
        TerminalClassification::Succeeded,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn changed_update_restarts_only_the_app_server() -> Result<(), Box<dyn std::error::Error>> {
    run_update_case(
        UpdateFixtureMode::Changed,
        TerminalClassification::Succeeded,
    )
    .await
}

#[tokio::test]
async fn explicit_host_restart_executes_the_requesting_cli_and_retains_host_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("explicit-host-restart")?;
    let router = PersistentRouterHealthFixture::start().await?;
    let coordination_paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let app_server_socket = directory.path().join("app.sock");
    let app_server_log = directory.path().join("app-pids.log");
    let replacement_receipt = directory.path().join("replacement.log");
    std::fs::write(&app_server_log, b"")?;

    let current_executable = std::env::current_exe()?;
    let mut host_process = tokio::process::Command::new(&current_executable);
    host_process
        .args([
            "--exact",
            "explicit_host_restart_child_entrypoint",
            "--nocapture",
        ])
        .env("CODEX_HOST_RESTART_CHILD", "1")
        .env(
            "CODEX_HOST_RESTART_OPERATOR_SOCKET",
            coordination_paths.operator_socket(),
        )
        .env(
            "CODEX_HOST_RESTART_INSTANCE_LOCK",
            coordination_paths.instance_lock(),
        )
        .env("CODEX_HOST_RESTART_ROUTER", router.address().to_string())
        .env("CODEX_HOST_RESTART_APP_SOCKET", &app_server_socket)
        .env("CODEX_HOST_RESTART_APP_LOG", &app_server_log)
        .env("CODEX_HOST_RESTART_RECEIPT", &replacement_receipt)
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut host_process = host_process.spawn()?;
    let original_host_process_id = host_process.id();
    let mut host_reaped = false;

    let proof_result: Result<(), Box<dyn std::error::Error>> = async {
        let original_host_process_id =
            original_host_process_id.ok_or("fixture Host PID is missing")?;
        let frames = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::RestartHost {
                executable: current_executable.clone(),
            },
            Duration::from_secs(20),
        )
        .await?;
        check(
            frames.iter().any(|frame| matches!(frame, OperatorFrame::Progress(codex_router_host::HostProgress::StoppingAppServer)))
                && frames.iter().any(|frame| matches!(frame, OperatorFrame::Progress(codex_router_host::HostProgress::ReExecuting))),
            &format!("explicit host restart must emit typed teardown and re-exec progress before old-host EOF: {frames:?}"),
        )?;
        let restart_started_at = tokio::time::Instant::now();
        let _replacement_pids = wait_for_process_ids(&app_server_log, 2).await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if tokio::net::UnixStream::connect(&app_server_socket).await.is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let restart_gap = restart_started_at.elapsed();
        eprintln!("whole_host_restart_socket_gap_ms={}", restart_gap.as_millis());
        check(
            restart_gap < Duration::from_secs(5),
            &format!("whole-Host restart socket gap exceeded target: {restart_gap:?}"),
        )?;
        host_process.kill().await?;
        let _output = tokio::time::timeout(Duration::from_secs(5), host_process.wait()).await??;
        host_reaped = true;
        assert_explicit_restart_receipt(
            &replacement_receipt,
            original_host_process_id,
            &current_executable,
        )?;
        let app_server_pid = wait_for_process_id(&app_server_log).await?;
        check(
            !process_is_running(app_server_pid),
            "explicit host restart must settle the old app-server before exec",
        )?;
        let reacquired = HostInstance::acquire(coordination_paths.clone())?;
        drop(reacquired);
        Ok(())
    }
    .await;

    let host_cleanup_result = if host_reaped {
        Ok(())
    } else {
        reap_fixture_host(&mut host_process).await
    };
    let child_cleanup_result = terminate_logged_fixture_processes(&[app_server_log.as_path()]);
    let router_cleanup_result = router.finish().await;
    finish_fixture_proof(
        proof_result,
        [
            host_cleanup_result,
            child_cleanup_result,
            router_cleanup_result,
        ],
    )
}

#[tokio::test]
async fn invalid_host_restart_executable_fails_before_child_teardown()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("invalid-host-restart")?;
    let router = PersistentRouterHealthFixture::start().await?;
    let managed_executable = directory.path().join("managed-codex");
    install_updater_fixture(&managed_executable, UpdateFixtureMode::NoChange)?;
    let app_server_socket = directory.path().join("app.sock");
    let app_server_log = directory.path().join("app-pids.log");
    std::fs::write(&app_server_log, b"")?;
    let current_executable = std::env::current_exe()?;
    let identity = codex_native_integration::executable_identity(&current_executable).await?;
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
            managed_executable,
            deadlines: fixture_host_deadlines()?,
        }),
        ManagedChildLaunchPlans::new(None, app_server),
        ManagedUpdateInputs::production(),
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
            "invalid-path fixture Host must start ready",
        )?;
        let app_server_pid = wait_for_process_id(&app_server_log).await?;
        let restart = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::RestartHost {
                executable: PathBuf::from("relative/codex-router"),
            },
            Duration::from_secs(20),
        )
        .await?;
        check_equal(
            terminal_classification(&restart)?,
            TerminalClassification::Failed,
            "relative replacement path must fail admission",
        )?;
        check(
            process_is_running(app_server_pid),
            "invalid replacement path must leave the retained app-server running",
        )?;
        let missing_projection = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::RestartHost {
                executable: current_executable,
            },
            Duration::from_secs(20),
        )
        .await?;
        check_equal(
            terminal_classification(&missing_projection)?,
            TerminalClassification::Failed,
            "missing Host-owned replacement projection must fail admission",
        )?;
        check(
            process_is_running(app_server_pid),
            "missing replacement projection must leave the retained app-server running",
        )?;
        let status = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::Status,
            Duration::from_secs(20),
        )
        .await?;
        check_equal(
            terminal_classification(&status)?,
            TerminalClassification::Ready,
            "invalid replacement path must leave operator readiness published",
        )
    }
    .await;

    runtime.abort();
    let _runtime_result = runtime.await;
    let child_cleanup_result = terminate_logged_fixture_processes(&[app_server_log.as_path()]);
    let router_cleanup_result = router.finish().await;
    finish_fixture_proof(proof_result, [child_cleanup_result, router_cleanup_result])
}

#[tokio::test]
async fn host_restart_exec_failure_releases_singleton_after_settling_children()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("host-restart-exec-failure")?;
    let router = PersistentRouterHealthFixture::start().await?;
    let invalid_executable = directory.path().join("invalid-codex-router");
    std::fs::write(&invalid_executable, b"#!/missing/interpreter\n")?;
    std::fs::set_permissions(&invalid_executable, std::fs::Permissions::from_mode(0o700))?;
    let current_executable = std::env::current_exe()?;
    let identity = codex_native_integration::executable_identity(&current_executable).await?;
    let app_server_socket = directory.path().join("app.sock");
    let app_server_log = directory.path().join("app-pids.log");
    std::fs::write(&app_server_log, b"")?;
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
    let mut runtime = tokio::spawn(HostRuntime::run(
        HostConfig::new(HostConfigInputs {
            coordination_paths: coordination_paths.clone(),
            router_endpoint: router.address(),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            app_server_socket,
            managed_executable: directory.path().join("unused-managed-codex"),
            deadlines: fixture_host_deadlines()?,
        }),
        ManagedChildLaunchPlans::new(None, app_server),
        ManagedUpdateInputs::production().with_replacement_command(ChildCommandSpec::new(
            PathBuf::from("/configured/old/codex-router"),
        )),
    ));

    let mut runtime_observed = false;
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
            "exec-failure fixture Host must start ready",
        )?;
        let app_server_pid = wait_for_process_id(&app_server_log).await?;
        let frames = send_operator_request(
            coordination_paths.operator_socket(),
            OperatorRequest::RestartHost {
                executable: invalid_executable,
            },
            Duration::from_secs(20),
        )
        .await?;
        check(
            frames
                .iter()
                .all(|frame| matches!(frame, OperatorFrame::Progress(_)))
                && frames.iter().any(|frame| {
                    matches!(
                        frame,
                        OperatorFrame::Progress(codex_router_host::HostProgress::ReExecuting)
                    )
                }),
            "failed exec must close the old connection after replacement progress",
        )?;
        let runtime_wait = tokio::time::timeout(Duration::from_secs(20), &mut runtime).await;
        runtime_observed = runtime_wait.is_ok();
        let runtime_result = runtime_wait??;
        check(
            matches!(runtime_result, Err(codex_router_host::HostError::Exec(_))),
            "failed replacement exec must terminate the foreground Host with an exec error",
        )?;
        check(
            !process_is_running(app_server_pid),
            "failed replacement exec must occur after app-server settlement",
        )?;
        let reacquired = HostInstance::acquire(coordination_paths.clone())?;
        drop(reacquired);
        Ok(())
    }
    .await;

    if !runtime_observed {
        runtime.abort();
        let _runtime_result = runtime.await;
    }
    let child_cleanup_result = terminate_logged_fixture_processes(&[app_server_log.as_path()]);
    let router_cleanup_result = router.finish().await;
    finish_fixture_proof(proof_result, [child_cleanup_result, router_cleanup_result])
}

#[tokio::test]
async fn explicit_host_restart_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_HOST_RESTART_CHILD").is_none() {
        return Ok(());
    }
    let current_executable = std::env::current_exe()?;
    let operator_socket = required_path("CODEX_HOST_RESTART_OPERATOR_SOCKET")?;
    let instance_lock = required_path("CODEX_HOST_RESTART_INSTANCE_LOCK")?;
    let app_server_socket = required_path("CODEX_HOST_RESTART_APP_SOCKET")?;
    let app_server_log = required_path("CODEX_HOST_RESTART_APP_LOG")?;
    let router_endpoint = std::env::var("CODEX_HOST_RESTART_ROUTER")?.parse()?;
    let identity = codex_native_integration::executable_identity(&current_executable).await?;
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
    let replacement = ChildCommandSpec::new(PathBuf::from("/configured/stale/codex-router"))
        .with_arguments([
            "--exact",
            "explicit_host_restart_replacement_child_entrypoint",
            "--nocapture",
        ])
        .with_environment("CODEX_HOST_RESTART_RETAINED", "retained-host-environment");
    let result = HostRuntime::run(
        HostConfig::new(HostConfigInputs {
            coordination_paths: HostCoordinationPaths::new(operator_socket, instance_lock),
            router_endpoint,
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            app_server_socket,
            managed_executable: PathBuf::from("/unused/managed-codex"),
            deadlines: fixture_host_deadlines()?,
        }),
        ManagedChildLaunchPlans::new(None, app_server),
        ManagedUpdateInputs::production().with_replacement_command(replacement),
    )
    .await;
    Err(format!("explicit host restart returned unexpectedly: {result:?}").into())
}

async fn wait_for_process_ids(
    process_log: &Path,
    expected_count: usize,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ids = std::fs::read_to_string(process_log)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.parse::<u32>().ok())
                .collect::<Vec<_>>();
            if ids.len() >= expected_count {
                return ids;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}

#[tokio::test]
async fn explicit_host_restart_replacement_child_entrypoint()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(marker) = std::env::var_os(inherited_lock_environment()) else {
        return Ok(());
    };
    let coordination_paths = HostCoordinationPaths::new(
        required_path("CODEX_HOST_RESTART_OPERATOR_SOCKET")?,
        required_path("CODEX_HOST_RESTART_INSTANCE_LOCK")?,
    );
    let owner = HostInstance::acquire_inherited(coordination_paths, &marker)?;
    std::fs::write(
        required_path("CODEX_HOST_RESTART_RECEIPT")?,
        format!(
            "{}\n{}\n{}\n",
            std::process::id(),
            std::env::current_exe()?.display(),
            std::env::var("CODEX_HOST_RESTART_RETAINED")?,
        ),
    )?;
    drop(owner);
    let app_server_socket = required_path("CODEX_HOST_RESTART_APP_SOCKET")?;
    let app_server_log = required_path("CODEX_HOST_RESTART_APP_LOG")?;
    run_native_app_server_fixture(
        Path::new(&app_server_socket),
        "1.2.3",
        Some(Path::new(&app_server_log)),
    )
    .await
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
    let identity = codex_native_integration::executable_identity(&current_executable).await?;
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
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
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
