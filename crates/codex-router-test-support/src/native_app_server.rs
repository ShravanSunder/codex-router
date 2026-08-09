//! Native app-server protocol fixtures for shared-host lifecycle proof.

use std::path::Path;
use std::time::Duration;

use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::net::UnixListener;
use tokio_tungstenite::tungstenite::Message;

/// Runs a multi-connection native app-server websocket fixture until SIGTERM.
pub async fn run_native_app_server_fixture(
    socket_path: &Path,
    running_version: &str,
    process_log: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let _stale_cleanup = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)?;
    if let Some(process_log) = process_log {
        append_event(process_log, &format!("{}\n", std::process::id()))?;
    }
    loop {
        tokio::select! {
            _ = terminate.recv() => break,
            accepted = listener.accept() => {
                let (stream, _peer) = accepted?;
                serve_native_app_server_observation(stream, running_version).await?;
            }
        }
    }
    drop(listener);
    let _socket_cleanup = std::fs::remove_file(socket_path);
    Ok(())
}

async fn serve_native_app_server_observation(
    stream: tokio::net::UnixStream,
    running_version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut websocket = tokio_tungstenite::accept_async(stream).await?;
    let initialize = read_fixture_json(&mut websocket).await?;
    let initialize_id = initialize
        .get("id")
        .and_then(Value::as_u64)
        .ok_or("initialize request id is missing")?;
    websocket
        .send(Message::Text(
            serde_json::json!({
                "id": initialize_id,
                "result": { "userAgent": format!("codex-cli/{running_version}") },
            })
            .to_string()
            .into(),
        ))
        .await?;
    let initialized = read_fixture_json(&mut websocket).await?;
    if initialized.get("method").and_then(Value::as_str) != Some("initialized") {
        return Err("initialized notification is missing".into());
    }
    let remote_status = read_fixture_json(&mut websocket).await?;
    let remote_status_id = remote_status
        .get("id")
        .and_then(Value::as_u64)
        .ok_or("Remote Control request id is missing")?;
    websocket
        .send(Message::Text(
            serde_json::json!({
                "id": remote_status_id,
                "result": {
                    "status": "connected",
                    "serverName": "shared-host-fixture",
                    "environmentId": "fixture-environment",
                },
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _close_frame = websocket.next().await;
    Ok(())
}

/// Serves one observation with independently delayed native and Remote Control stages.
pub async fn run_delayed_native_app_server_observation_fixture(
    socket_path: &Path,
    running_version: &str,
    native_delay: Duration,
    remote_control_delay: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = UnixListener::bind(socket_path)?;
    let (stream, _peer) = listener.accept().await?;
    let mut websocket = tokio_tungstenite::accept_async(stream).await?;
    let initialize = read_fixture_json_send(&mut websocket).await?;
    let initialize_id = initialize
        .get("id")
        .cloned()
        .ok_or("initialize request id is missing")?;
    tokio::time::sleep(native_delay).await;
    websocket
        .send(Message::Text(
            serde_json::json!({
                "id": initialize_id,
                "result": { "userAgent": format!("codex-cli/{running_version}") }
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _initialized = read_fixture_json_send(&mut websocket).await?;
    let remote_status = read_fixture_json_send(&mut websocket).await?;
    let remote_status_id = remote_status
        .get("id")
        .cloned()
        .ok_or("Remote Control request id is missing")?;
    websocket
        .send(Message::Text(
            serde_json::json!({
                "id": remote_status_id,
                "result": {
                    "status": "connecting",
                    "serverName": "fixture",
                    "environmentId": "fixture"
                }
            })
            .to_string()
            .into(),
        ))
        .await?;
    tokio::time::sleep(remote_control_delay).await;
    websocket
        .send(Message::Text(
            serde_json::json!({
                "method": "remoteControl/status/changed",
                "params": {
                    "status": "connected",
                    "serverName": "fixture",
                    "environmentId": "fixture"
                }
            })
            .to_string()
            .into(),
        ))
        .await?;
    Ok(())
}

async fn read_fixture_json_send(
    websocket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let message = websocket
            .next()
            .await
            .ok_or("native websocket closed unexpectedly")??;
        if let Message::Text(text) = message {
            return Ok(serde_json::from_str(&text)?);
        }
    }
}

async fn read_fixture_json(
    websocket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        let message = websocket
            .next()
            .await
            .ok_or("native websocket closed unexpectedly")??;
        if let Message::Text(text) = message {
            return Ok(serde_json::from_str(&text)?);
        }
    }
}

fn append_event(event_file: &Path, event: &str) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new().append(true).open(event_file)?;
    file.write_all(event.as_bytes())
}
