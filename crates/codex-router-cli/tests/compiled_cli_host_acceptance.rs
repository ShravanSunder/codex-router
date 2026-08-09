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
async fn compiled_cli_runs_status_restart_and_direct_session_attachment()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new()?;
    let router_root = directory.path().join("router");
    let codex_home = directory.path().join("codex");
    let socket_path = directory.path().join("debug-app-server.sock");
    let managed_executable = codex_home.join("packages/standalone/current/codex");
    std::fs::create_dir_all(
        managed_executable
            .parent()
            .ok_or("managed executable parent is missing")?,
    )?;
    install_managed_fixture(&managed_executable)?;
    let port = reserve_loopback_port()?;
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_codex-router"));

    let mut host = tokio::process::Command::new(&binary);
    host.args([
        "host",
        "--router-root",
        router_root.to_str().ok_or("router root is not UTF-8")?,
        "--port",
        &port.to_string(),
    ])
    .env("CODEX_HOME", &codex_home)
    .env("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET", &socket_path)
    .env("HOME", directory.path())
    .env(
        "CODEX_ROUTER_COMPILED_CLI_TEST_BINARY",
        std::env::current_exe()?,
    )
    .env("CODEX_ROUTER_COMPILED_CLI_APP_CHILD", "1")
    .env("CODEX_ROUTER_COMPILED_CLI_UPDATE_CHANGES", "1")
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    let host = host.spawn()?;
    wait_for_operator_socket(&router_root.join("host.sock")).await?;

    let status =
        run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, "status").await?;
    check(
        status.status.success(),
        &String::from_utf8_lossy(&status.stderr),
    )?;
    let status_stdout = String::from_utf8(status.stdout)?;
    check(status_stdout.contains("readiness: Ready"), &status_stdout)?;
    check(
        status_stdout.contains("remote_control: Connected"),
        &status_stdout,
    )?;

    let restart =
        run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, "restart").await?;
    check(
        restart.status.success(),
        &String::from_utf8_lossy(&restart.stderr),
    )?;
    check(
        String::from_utf8(restart.stdout)?.contains("result: Succeeded"),
        "app-server restart did not report success",
    )?;

    let update =
        run_host_subcommand(&binary, &router_root, &codex_home, &socket_path, "update").await?;
    check(
        update.status.success(),
        &String::from_utf8_lossy(&update.stderr),
    )?;
    let update_stdout = String::from_utf8(update.stdout)?;
    check(
        update_stdout.contains("update_result: updated and host restarted"),
        &update_stdout,
    )?;

    let sessions = tokio::process::Command::new(&binary)
        .args(["sessions", "--new", "--dry-run"])
        .env("CODEX_HOME", &codex_home)
        .env("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET", &socket_path)
        .env("HOME", directory.path())
        .output()
        .await?;
    check(
        sessions.status.success(),
        &String::from_utf8_lossy(&sessions.stderr),
    )?;
    let sessions_stdout = String::from_utf8(sessions.stdout)?;
    check(
        sessions_stdout.contains(&format!("--remote unix://{}", socket_path.display())),
        &sessions_stdout,
    )?;

    let host_process_id = host.id().ok_or("host process ID is unavailable")?;
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(i32::try_from(host_process_id)?)
            .ok_or("host process ID is zero")?,
        rustix::process::Signal::INT,
    )?;
    let output = tokio::time::timeout(Duration::from_secs(5), host.wait_with_output()).await??;
    check(
        output.status.success(),
        &String::from_utf8_lossy(&output.stderr),
    )?;
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
    let socket_path = PathBuf::from(
        std::env::var_os("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET")
            .ok_or("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET missing")?,
    );
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
        .and_then(Value::as_u64)
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
        .and_then(Value::as_u64)
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
            .env("CODEX_HOME", codex_home)
            .env("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET", app_server_socket)
            .output(),
    )
    .await??)
}

async fn wait_for_operator_socket(socket: &Path) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

fn install_managed_fixture(executable: &Path) -> std::io::Result<()> {
    std::fs::write(
        executable,
        b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nif [ \"$1\" = \"update\" ]; then if [ \"$CODEX_ROUTER_COMPILED_CLI_UPDATE_CHANGES\" = \"1\" ]; then printf '\\n# changed by update fixture\\n' >> \"$0\"; fi; exit 0; fi\nexec \"$CODEX_ROUTER_COMPILED_CLI_TEST_BINARY\" --exact compiled_cli_app_server_child_entrypoint --nocapture\n",
    )?;
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
