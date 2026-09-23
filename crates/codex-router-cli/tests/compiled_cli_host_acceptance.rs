use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

#[path = "support/host_replacement_observation.rs"]
mod host_replacement_observation;
use host_replacement_observation::{binary_version, observe_continuous_lock, verify_host_image};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn installed_mode_rejects_host_before_publication_when_fixture_launchctl_fails()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new()?;
    let router_root = directory.path().join("router");
    let codex_home = directory.path().join("codex");
    let socket_path = codex_native_integration::CodexPaths::from_codex_home(codex_home.clone())
        .app_server_socket();
    let launchctl_executable = directory.path().join("launchctl");
    install_rejected_launchctl_fixture(&launchctl_executable)?;
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_codex-router"));

    let output = tokio::time::timeout(
        Duration::from_secs(12),
        tokio::process::Command::new(&binary)
            .args([
                "host",
                "--router-root",
                router_root.to_str().ok_or("router root is not UTF-8")?,
            ])
            // Installed-mode behavior is tested only with fake HOME, Codex and launchctl.
            .env("CODEX_ROUTER_USE_HOME_DEFAULT", "1")
            .env("OTEL_SDK_DISABLED", "true")
            .kill_on_drop(true)
            .env("CODEX_HOME", &codex_home)
            .env("CODEX_ROUTER_COMPILED_CLI_NATIVE_SOCKET", &socket_path)
            .env("CODEX_ROUTER_DEBUG_LAUNCHCTL", &launchctl_executable)
            .env("HOME", directory.path())
            .output(),
    )
    .await??;

    check(!output.status.success(), "host unexpectedly started")?;
    check(
        String::from_utf8(output.stderr)?
            .contains("launchctl rejected Codex Desktop local-daemon attachment"),
        "host did not report the launch-session policy failure",
    )?;
    check(
        !router_root.join("host.sock").exists(),
        "operator socket was published before launch-session policy succeeded",
    )?;
    Ok(())
}

#[tokio::test]
async fn installed_cli_restarts_host_from_new_install_path()
-> Result<(), Box<dyn std::error::Error>> {
    run_host_install_journey(false).await
}

#[tokio::test]
async fn installed_cli_restarts_host_after_atomic_binary_install()
-> Result<(), Box<dyn std::error::Error>> {
    run_host_install_journey(true).await
}

async fn run_host_install_journey(atomic_install: bool) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new()?;
    let router_root = directory.path().join("router");
    let codex_home = directory.path().join("codex");
    let socket_path = codex_native_integration::CodexPaths::from_codex_home(codex_home.clone())
        .app_server_socket();
    let managed_executable = codex_home.join("packages/standalone/current/codex");
    let launchctl_executable = directory.path().join("launchctl");
    let curl_executable = directory.path().join("curl");
    let launchctl_log = directory.path().join("launchctl.log");
    let updater_failure_marker = directory.path().join("updater-failure-marker");
    let curl_failure_marker = directory.path().join("curl-failure-marker");
    let process_log = directory.path().join("app-generations.log");
    std::fs::create_dir_all(
        managed_executable
            .parent()
            .ok_or("managed executable parent is missing")?,
    )?;
    install_managed_fixture(&managed_executable)?;
    install_launchctl_fixture(&launchctl_executable)?;
    install_curl_fixture(&curl_executable)?;
    let port = reserve_loopback_port()?;
    let candidate_binary = PathBuf::from(env!("CARGO_BIN_EXE_codex-router"));
    let prior_binary = std::env::var_os("CODEX_ROUTER_RESTART_PRIOR_BINARY").map(PathBuf::from);
    if let Some(prior) = &prior_binary {
        let before = binary_version(prior).await?;
        let after = binary_version(&candidate_binary).await?;
        check(
            before != after,
            "cross-version proof requires different package versions",
        )?;
        eprintln!(
            "cross-version candidate handoff: {before} -> {after}; atomic_install={atomic_install}"
        );
    }
    let old_install = directory.path().join("old-install");
    let new_install = directory.path().join("new-install");
    std::fs::create_dir_all(&old_install)?;
    std::fs::create_dir_all(&new_install)?;
    let binary = old_install.join("codex-router");
    std::fs::copy(
        prior_binary.as_deref().unwrap_or(&candidate_binary),
        &binary,
    )?;
    let replacement_binary = if atomic_install {
        binary.clone()
    } else {
        new_install.join("codex-router")
    };

    let host_stdout = directory.path().join("host-stdout.log");
    let host_stderr = directory.path().join("host-stderr.log");
    let mut host = tokio::process::Command::new(&binary);
    host.args([
        "host",
        "--router-root",
        router_root.to_str().ok_or("router root is not UTF-8")?,
        "--port",
        &port.to_string(),
        "--mcp-bind",
        "127.0.0.1:0",
    ])
    .env("CODEX_ROUTER_USE_HOME_DEFAULT", "1")
    .env("OTEL_SDK_DISABLED", "true")
    .kill_on_drop(true)
    .env("CODEX_HOME", &codex_home)
    .env("CODEX_ROUTER_COMPILED_CLI_NATIVE_SOCKET", &socket_path)
    .env("CODEX_ROUTER_DEBUG_LAUNCHCTL", &launchctl_executable)
    .env("CODEX_ROUTER_COMPILED_CLI_LAUNCHCTL_LOG", &launchctl_log)
    .env("HOME", directory.path())
    .env(
        "CODEX_ROUTER_COMPILED_CLI_TEST_BINARY",
        std::env::current_exe()?,
    )
    .env("CODEX_ROUTER_COMPILED_CLI_APP_CHILD", "1")
    .env("CODEX_ROUTER_COMPILED_CLI_PROCESS_LOG", &process_log)
    .env("CODEX_ROUTER_COMPILED_CLI_UPDATE_CHANGES", "1")
    .env(
        "CODEX_ROUTER_COMPILED_CLI_UPDATE_FAILURE_MARKER",
        &updater_failure_marker,
    )
    .env(
        "CODEX_ROUTER_COMPILED_CLI_CURL_FAILURE_MARKER",
        &curl_failure_marker,
    )
    .env("CODEX_ROUTER_COMPILED_CLI_MANAGED", &managed_executable)
    .env(
        "PATH",
        prepend_path(curl_executable.parent().ok_or("curl parent is missing")?)?,
    )
    .env("CODEX_ROUTER_DEBUG_READINESS_TIMING", "1")
    .stdout(Stdio::from(std::fs::File::create(&host_stdout)?))
    .stderr(Stdio::from(std::fs::File::create(&host_stderr)?));
    let mut host = host.spawn()?;
    // Always release owned children, including when an assertion below fails.
    let proof = async {
        wait_for_operator_socket(&mut host, &router_root.join("host.sock"), &host_stderr).await?;
        let timing_output = std::fs::read_to_string(&host_stderr)?;
        for stage in ["launchctlPolicy", "executableIdentity"] {
            check(
                timing_output.contains(stage),
                &format!("missing debug readiness timing stage {stage}: {timing_output}"),
            )?;
        }
        eprintln!("compiled_acceptance_readiness_timing={timing_output}");
        check(
            std::fs::read_to_string(&launchctl_log)?.trim()
                == "setenv CODEX_APP_SERVER_USE_LOCAL_DAEMON 1",
            "foreground host did not configure Desktop local-daemon attachment",
        )?;

        let status =
            run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, &["status"]).await?;
        check(
            status.status.success(),
            &format!("status: {}", String::from_utf8_lossy(&status.stderr)),
        )?;
        let status_stdout = String::from_utf8(status.stdout)?;
        check(status_stdout.contains("readiness: ready"), &status_stdout)?;
        check(
            status_stdout.contains("remote_control: connected"),
            &status_stdout,
        )?;
        check(
            status_stdout.contains("remote_server_name: cli-smoke"),
            &status_stdout,
        )?;
        check(
            status_stdout.contains("remote_environment_id: cli-smoke"),
            &status_stdout,
        )?;
        check(
            status_stdout.contains("desktop attachment: configured"),
            &status_stdout,
        )?;
        check(
            status_stdout.contains("desktop relaunch: restart required if already running"),
            &status_stdout,
        )?;

        let original_host_pid = host.id().ok_or("owned Host PID missing")?;
        verify_host_image(original_host_pid, &binary)?;
        check(std::fs::read_to_string(&process_log)?.lines().count() == 1, "initial app-server generation missing")?;
        let restart =
            run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, &["app-server", "restart"])
                .await?;
        check(
            restart.status.success(),
            &format!("restart: {}", String::from_utf8_lossy(&restart.stderr)),
        )?;
        check(
            String::from_utf8(restart.stdout)?.contains("result: succeeded"),
            "app-server restart did not report success",
        )?;

        check(std::fs::read_to_string(&launchctl_log)?.lines().count() == 1, "app-server-only restart replaced the Host")?;
        check(std::fs::read_to_string(&process_log)?.lines().count() == 2, "app-server restart did not replace exactly one child")?;
        verify_host_image(original_host_pid, &binary)?;

        let update =
            run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, &["app-server", "update"]).await?;
        check(
            update.status.success(),
            &format!("update: {}", String::from_utf8_lossy(&update.stderr)),
        )?;
        let update_stdout = String::from_utf8(update.stdout)?;
        check(
            update_stdout.contains("update_result: updated and app-server restarted"),
            &update_stdout,
        )?;

        check(std::fs::read_to_string(&launchctl_log)?.lines().count() == 1, "app-server update replaced the Host")?;
        check(std::fs::read_to_string(&process_log)?.lines().count() == 3, "changed update child generation missing")?;
        std::fs::write(&updater_failure_marker, b"fail")?;
        let failed_update =
            run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, &["app-server", "update"]).await?;
        check(!failed_update.status.success(), "failed updater returned a successful CLI exit status")?;
        check(
            String::from_utf8(failed_update.stdout)?.contains("update_result: update failed without restart"),
            "failed updater was not classified before child replacement",
        )?;
        check(std::fs::read_to_string(&process_log)?.lines().count() == 3, "failed updater replaced the app-server child")?;
        std::fs::remove_file(&updater_failure_marker)?;
        std::fs::write(&curl_failure_marker, b"fail")?;
        let failed_download =
            run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, &["app-server", "update"]).await?;
        check(!failed_download.status.success(), "failed installer download returned a successful CLI exit status")?;
        check(
            String::from_utf8(failed_download.stdout)?.contains("update_result: update failed without restart"),
            "failed installer download was reported as no change",
        )?;
        check(std::fs::read_to_string(&process_log)?.lines().count() == 3, "failed download replaced the app-server child")?;
        std::fs::remove_file(&curl_failure_marker)?;
        let host_process_id = host.id().ok_or("owned Host PID missing")?;
        verify_host_image(host_process_id, &binary)?;
        let lock_path = router_root.join("host.lock");
        let lock_inode = std::fs::metadata(&lock_path)?.ino();
        let candidate_install = directory.path().join("candidate-install");
        std::fs::copy(&candidate_binary, &candidate_install)?;
        let old_inode = std::fs::metadata(&binary)?.ino();
        std::fs::rename(&candidate_install, &replacement_binary)?;
        check(std::fs::metadata(&replacement_binary)?.ino() != old_inode, "replacement must be a distinct executable identity")?;

        let (finish_sender, finish_receiver) = tokio::sync::oneshot::channel();
        let contention = tokio::spawn(observe_continuous_lock(lock_path.clone(), finish_receiver));
        let restart_result = run_host_subcommand(
            &replacement_binary, &router_root, &codex_home, &socket_path, &["restart"],
        ).await;
        let _finished = finish_sender.send(());
        contention.await??;
        let restarted = restart_result?;
        check(restarted.status.success(), &format!("whole Host restart: {}", String::from_utf8_lossy(&restarted.stderr)))?;
        let restarted_stdout = String::from_utf8(restarted.stdout)?;
        check(restarted_stdout.contains("restart_result: host restarted using installed executable"), &restarted_stdout)?;
        check(restarted_stdout.contains("readiness: ready"), &restarted_stdout)?;
        check(host.try_wait()?.is_none(), "original Host PID exited instead of replacing its image")?;
        verify_host_image(host_process_id, &replacement_binary)?;
        check(std::fs::metadata(&lock_path)?.ino() == lock_inode, "Host replaced its stable lock artifact")?;
        check(
            std::fs::metadata(&host_stdout)?.len() == 0,
            "foreground Host printed startup or re-exec progress to its terminal",
        )?;
        check(std::fs::read_to_string(&launchctl_log)?.lines().count() == 2, "whole Host restart did not activate once")?;
        let generations = std::fs::read_to_string(&process_log)?;
        check(generations.lines().count() == 4, "whole Host restart child generation missing")?;
        for former_pid in generations.lines().take(3) {
            let former_pid = rustix::process::Pid::from_raw(former_pid.parse()?).ok_or("invalid child PID")?;
            check(matches!(rustix::process::test_kill_process(former_pid), Err(rustix::io::Errno::SRCH)), "an old app-server child is still present or its absence could not be verified")?;
        }
        let child_environment = std::fs::read_to_string(process_log.with_extension("handoff"))?;
        check(child_environment.lines().last().is_some_and(|line| line.ends_with(" false")), "replacement app-server inherited the Host-only handoff marker")?;
        eprintln!("Host PID {host_process_id} now maps installed image {}; stable lock retained; four child generations settled; ordinary child handoff marker absent", replacement_binary.display());

        let service_directory = router_root.join("agent-communication");
        let native_path = tokio::task::spawn_blocking(move || {
            collaboration_client::resolve_public_native(&service_directory)
        })
        .await??;
        check(
            native_path
                == std::fs::canonicalize(
                    router_root.join("agent-communication/codex-native.sock"),
                )?,
            "Sessions SDK selector must resolve the public native relay",
        )?;
        let mut native =
            codex_native_integration::NativeProtocolConnection::connect(&native_path).await?;
        let remote = native
            .request("remoteControl/status", serde_json::json!({}))
            .await?;
        check(
            remote.get("serverName").and_then(Value::as_str) == Some("cli-smoke"),
            "public native attachment must reach the same managed backend",
        )?;
        drop(native);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    if host.try_wait()?.is_none() {
        let host_process_id = host.id().ok_or("host process ID is unavailable")?;
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(i32::try_from(host_process_id)?)
                .ok_or("host process ID is zero")?,
            rustix::process::Signal::INT,
        )?;
    }
    let status = tokio::time::timeout(Duration::from_secs(5), host.wait())
        .await
        .map_err(|_elapsed| {
            std::io::Error::other(format!(
                "fixture Host did not exit after INT; proof={}; stderr={}",
                proof
                    .as_ref()
                    .err()
                    .map_or("ok".to_owned(), ToString::to_string),
                std::fs::read_to_string(&host_stderr).unwrap_or_default()
            ))
        })??;
    proof.map_err(|error| {
        std::io::Error::other(format!(
            "{error}; fixture Host stderr: {}",
            std::fs::read_to_string(&host_stderr).unwrap_or_default()
        ))
    })?;
    check(status.success(), &std::fs::read_to_string(&host_stderr)?)?;
    check(
        !router_root.join("host.sock").exists(),
        "operator socket leaked",
    )?;
    check(
        router_root.join("host.lock").exists(),
        "stable lock artifact missing",
    )?;
    Ok(())
}

#[tokio::test]
async fn compiled_cli_app_server_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_COMPILED_CLI_APP_CHILD").is_none() {
        return Ok(());
    }
    if std::env::var_os("CODEX_ROUTER_COMPILED_CLI_EMIT_CHILD_DIAGNOSTICS").is_some() {
        eprintln!(
            "OAuth refresh token was rejected: invalid_grant: CODEX_ROUTER_CHILD_SECRET_CANARY"
        );
        eprintln!("failed to refresh available models: missing field `display_name`");
    }
    if let Some(log) = std::env::var_os("CODEX_ROUTER_COMPILED_CLI_PROCESS_LOG") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)?;
        std::io::Write::write_all(&mut file, format!("{}\n", std::process::id()).as_bytes())?;
        let mut environment_log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(Path::new(&log).with_extension("handoff"))?;
        std::io::Write::write_all(
            &mut environment_log,
            format!(
                "{} {}\n",
                std::process::id(),
                std::env::var_os(codex_router_host::inherited_lock_environment()).is_some()
            )
            .as_bytes(),
        )?;
    }
    let socket_path = PathBuf::from(
        std::env::var_os("CODEX_ROUTER_COMPILED_CLI_NATIVE_SOCKET")
            .ok_or("CODEX_ROUTER_COMPILED_CLI_NATIVE_SOCKET missing")?,
    );
    std::fs::create_dir_all(
        socket_path
            .parent()
            .ok_or("fixture socket parent missing")?,
    )?;
    let _stale_cleanup = std::fs::remove_file(&socket_path);
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            _ = terminate.recv() => break,
            accepted = listener.accept() => {
                let (stream, _peer) = accepted?;
                serve_app_server_observation(stream).await?;
            }
        }
    }
    drop(listener);
    let _cleanup = std::fs::remove_file(socket_path);
    Ok(())
}

async fn serve_app_server_observation(
    stream: tokio::net::UnixStream,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut websocket = tokio_tungstenite::accept_async(stream).await?;
    let initialize = read_json(&mut websocket).await?;
    let initialize_id = initialize
        .get("id")
        .filter(|id| id.is_string() || id.as_i64().is_some())
        .ok_or("initialize id missing")?;
    websocket
        .send(Message::Text(
            serde_json::json!({
                "id": initialize_id,
                "result": { "userAgent": "codex-cli/1.2.3" }
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _initialized = read_json(&mut websocket).await?;
    let remote_status = read_json(&mut websocket).await?;
    let remote_status_id = remote_status
        .get("id")
        .filter(|id| id.is_string() || id.as_i64().is_some())
        .ok_or("remote status id missing")?;
    websocket
        .send(Message::Text(
            serde_json::json!({
                "id": remote_status_id,
                "result": {
                    "status": "connected",
                    "serverName": "cli-smoke",
                    "environmentId": "cli-smoke"
                }
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _closed = websocket.next().await;
    Ok(())
}

async fn read_json(
    websocket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        let message = websocket.next().await.ok_or("websocket closed")??;
        if let Message::Text(text) = message {
            return Ok(serde_json::from_str(&text)?);
        }
    }
}

async fn run_host_subcommand(
    binary: &Path,
    router_root: &Path,
    codex_home: &Path,
    app_server_socket: &Path,
    subcommand: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(
        Duration::from_secs(12),
        tokio::process::Command::new(binary)
            .arg("host")
            .args(subcommand)
            .arg("--router-root")
            .arg(router_root)
            .env("CODEX_ROUTER_USE_HOME_DEFAULT", "1")
            .env("OTEL_SDK_DISABLED", "true")
            .env(
                "HOME",
                codex_home.parent().ok_or("fixture HOME is missing")?,
            )
            .kill_on_drop(true)
            .env("CODEX_HOME", codex_home)
            .env("CODEX_ROUTER_COMPILED_CLI_NATIVE_SOCKET", app_server_socket)
            .output(),
    )
    .await??)
}

async fn wait_for_operator_socket(
    host: &mut tokio::process::Child,
    socket: &Path,
    stderr: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(12), async {
        while !socket.exists() {
            if let Some(status) = host.try_wait()? {
                return Err(std::io::Error::other(format!(
                    "fixture Host exited {status}: {}",
                    std::fs::read_to_string(stderr)?
                )));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        while !std::fs::read_to_string(stderr)
            .map(|contents| contents.contains("executableIdentity"))
            .unwrap_or(false)
        {
            if let Some(status) = host.try_wait()? {
                return Err(std::io::Error::other(format!(
                    "fixture Host exited {status}: {}",
                    std::fs::read_to_string(stderr)?
                )));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), std::io::Error>(())
    })
    .await??;
    Ok(())
}

fn install_managed_fixture(executable: &Path) -> std::io::Result<()> {
    std::fs::write(
        executable,
        b"#!/bin/sh\nif [ \"$2\" = \"generate-json-schema\" ]; then exit 1; fi\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nif [ \"$1\" = \"update\" ]; then if [ \"$CODEX_ROUTER_COMPILED_CLI_UPDATE_CHANGES\" = \"1\" ]; then printf '\\n# changed by update fixture\\n' >> \"$0\"; fi; exit 0; fi\nexec \"$CODEX_ROUTER_COMPILED_CLI_TEST_BINARY\" --exact compiled_cli_app_server_child_entrypoint --nocapture\n",
    )?;
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700))
}

fn install_launchctl_fixture(executable: &Path) -> std::io::Result<()> {
    std::fs::write(
        executable,
        b"#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$CODEX_ROUTER_COMPILED_CLI_LAUNCHCTL_LOG\"\n",
    )?;
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700))
}

fn install_curl_fixture(executable: &Path) -> std::io::Result<()> {
    std::fs::write(
        executable,
        b"#!/bin/sh\n[ ! -e \"$CODEX_ROUTER_COMPILED_CLI_CURL_FAILURE_MARKER\" ] || exit 22\ncat <<'INSTALLER'\n#!/bin/sh\n[ \"$CODEX_NON_INTERACTIVE\" = \"1\" ] || exit 43\n[ ! -e \"$CODEX_ROUTER_COMPILED_CLI_UPDATE_FAILURE_MARKER\" ] || exit 43\nif [ \"$CODEX_ROUTER_COMPILED_CLI_UPDATE_CHANGES\" = \"1\" ]; then\n  printf '\\n# changed by update fixture\\n' >> \"$CODEX_ROUTER_COMPILED_CLI_MANAGED\"\nfi\nINSTALLER\n",
    )?;
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700))
}

fn prepend_path(directory: &Path) -> std::io::Result<std::ffi::OsString> {
    let existing =
        std::env::var_os("PATH").ok_or_else(|| std::io::Error::other("PATH is missing"))?;
    let mut paths = std::env::split_paths(&existing).collect::<Vec<_>>();
    paths.insert(0, directory.to_owned());
    std::env::join_paths(paths).map_err(std::io::Error::other)
}

fn install_rejected_launchctl_fixture(executable: &Path) -> std::io::Result<()> {
    std::fs::write(executable, b"#!/bin/sh\nexit 1\n")?;
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700))
}

fn reserve_loopback_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
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
    fn new() -> std::io::Result<Self> {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("/tmp").join(format!("crh-{}-{counter}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.path);
    }
}
