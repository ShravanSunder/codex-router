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
async fn installed_mode_runs_status_restart_update_and_public_native_attachment()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new()?;
    let router_root = directory.path().join("router");
    let codex_home = directory.path().join("codex");
    let socket_path = codex_native_integration::CodexPaths::from_codex_home(codex_home.clone())
        .app_server_socket();
    let managed_executable = codex_home.join("packages/standalone/current/codex");
    let launchctl_executable = directory.path().join("launchctl");
    let launchctl_log = directory.path().join("launchctl.log");
    std::fs::create_dir_all(
        managed_executable
            .parent()
            .ok_or("managed executable parent is missing")?,
    )?;
    install_managed_fixture(&managed_executable)?;
    install_launchctl_fixture(&launchctl_executable)?;
    let port = reserve_loopback_port()?;
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_codex-router"));

    let host_stderr = directory.path().join("host-stderr.log");
    let mut host = tokio::process::Command::new(&binary);
    host.args([
        "host",
        "--router-root",
        router_root.to_str().ok_or("router root is not UTF-8")?,
        "--port",
        &port.to_string(),
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
    .env("CODEX_ROUTER_COMPILED_CLI_UPDATE_CHANGES", "1")
    .stdout(Stdio::null())
    .stderr(Stdio::from(std::fs::File::create(&host_stderr)?));
    let mut host = host.spawn()?;
    // Always release owned children, including when an assertion below fails.
    let proof = async {
        wait_for_operator_socket(&mut host, &router_root.join("host.sock"), &host_stderr).await?;
        check(
            std::fs::read_to_string(&launchctl_log)?.trim()
                == "setenv CODEX_APP_SERVER_USE_LOCAL_DAEMON 1",
            "foreground host did not configure Desktop local-daemon attachment",
        )?;

        let status =
            run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, "status").await?;
        check(
            status.status.success(),
            &format!("status: {}", String::from_utf8_lossy(&status.stderr)),
        )?;
        let status_stdout = String::from_utf8(status.stdout)?;
        check(status_stdout.contains("readiness: Ready"), &status_stdout)?;
        check(
            status_stdout.contains("remote_control: Connected"),
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
            status_stdout.contains("desktop_attachment: Configured"),
            &status_stdout,
        )?;
        check(
            status_stdout.contains("desktop_relaunch: required_if_running"),
            &status_stdout,
        )?;

        let restart =
            run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, "restart")
                .await?;
        check(
            restart.status.success(),
            &format!("restart: {}", String::from_utf8_lossy(&restart.stderr)),
        )?;
        check(
            String::from_utf8(restart.stdout)?.contains("result: Succeeded"),
            "app-server restart did not report success",
        )?;

        let update =
            run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, "update").await?;
        check(
            update.status.success(),
            &format!("update: {}", String::from_utf8_lossy(&update.stderr)),
        )?;
        let update_stdout = String::from_utf8(update.stdout)?;
        check(
            update_stdout.contains("update_result: updated and host restarted"),
            &update_stdout,
        )?;

        let service_directory = router_root.join("agent-communication");
        let native_path = tokio::task::spawn_blocking(move || {
            communication_client::resolve_public_native(&service_directory)
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
    let status = tokio::time::timeout(Duration::from_secs(5), host.wait()).await??;
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
    subcommand: &str,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(
        Duration::from_secs(12),
        tokio::process::Command::new(binary)
            .args([
                "host",
                subcommand,
                "--router-root",
                router_root.to_str().ok_or("router root is not UTF-8")?,
            ])
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
        b"#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$CODEX_ROUTER_COMPILED_CLI_LAUNCHCTL_LOG\"\n",
    )?;
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700))
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
