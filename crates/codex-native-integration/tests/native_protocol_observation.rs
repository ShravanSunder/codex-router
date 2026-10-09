use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_native_integration::RemoteControlObservation;
use codex_native_integration::observe_app_server;
use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::net::UnixListener;
use tokio_tungstenite::tungstenite::Message;

static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn native_probe_initializes_experimental_api_and_waits_for_remote_connection() {
    let socket = TestSocket::new("connected")
        .unwrap_or_else(|error| panic!("native fixture directory should create: {error}"));
    let listener = UnixListener::bind(socket.path())
        .unwrap_or_else(|error| panic!("native fixture socket should bind: {error}"));
    let server = tokio::spawn(async move {
        let (stream, _address) = listener
            .accept()
            .await
            .unwrap_or_else(|error| panic!("native fixture should accept: {error}"));
        let mut websocket = tokio_tungstenite::accept_async(stream)
            .await
            .unwrap_or_else(|error| panic!("native fixture should upgrade: {error}"));

        let initialize = next_json(&mut websocket)
            .await
            .unwrap_or_else(|error| panic!("initialize request should decode: {error}"));
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(
            initialize["params"]["capabilities"]["experimentalApi"],
            true
        );
        websocket
            .send(Message::Text(
                serde_json::json!({
                    "id": initialize["id"],
                    "result": {
                        "userAgent": "codex_app_server_daemon/1.2.3 (macOS; arm64) codex_cli_rs/1.2.3"
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap_or_else(|error| panic!("initialize response should send: {error}"));

        let initialized = next_json(&mut websocket)
            .await
            .unwrap_or_else(|error| panic!("initialized notification should decode: {error}"));
        assert_eq!(initialized["method"], "initialized");
        let status_read = next_json(&mut websocket)
            .await
            .unwrap_or_else(|error| panic!("status request should decode: {error}"));
        assert_eq!(status_read["method"], "remoteControl/status/read");
        websocket
            .send(Message::Text(
                remote_status_response(&status_read["id"], "connecting")
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap_or_else(|error| panic!("status response should send: {error}"));
        websocket
            .send(Message::Text(
                serde_json::json!({
                    "method": "remoteControl/status/changed",
                    "params": remote_status("connected"),
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap_or_else(|error| panic!("status notification should send: {error}"));
    });

    let observation = observe_app_server(
        socket.path(),
        Duration::from_secs(1),
        Duration::from_secs(1),
    )
    .await
    .unwrap_or_else(|error| panic!("native app-server should be observed: {error}"));

    assert_eq!(observation.running_version(), "1.2.3");
    assert_eq!(
        observation.remote_control(),
        &RemoteControlObservation::Connected {
            server_name: "owner-mac".to_owned(),
            environment_id: Some("env_123".to_owned()),
        }
    );
    server
        .await
        .unwrap_or_else(|error| panic!("native fixture task should join: {error}"));
}

#[tokio::test]
async fn native_protocol_preserves_empty_remote_control_names_and_present_empty_environment_ids() {
    let observation = observe_direct_remote_status(
        remote_status_with_claims("connected", "", Some("")),
        "empty-remote-name",
    )
    .await
    .expect("native codec should preserve empty raw claims");

    assert_eq!(
        observation.remote_control(),
        &RemoteControlObservation::Connected {
            server_name: String::new(),
            environment_id: Some(String::new()),
        }
    );
}

#[tokio::test]
async fn native_protocol_preserves_unusual_remote_control_names_and_missing_environment_ids() {
    let observation = observe_direct_remote_status(
        remote_status_with_claims("errored", "Remote\n\"Name\"", None),
        "unusual-remote-name",
    )
    .await
    .expect("native codec should preserve unusual raw claims");

    assert_eq!(
        observation.remote_control(),
        &RemoteControlObservation::Errored {
            server_name: "Remote\n\"Name\"".to_owned(),
            environment_id: None,
        }
    );
}

async fn observe_direct_remote_status(
    remote_status: Value,
    socket_name: &str,
) -> Result<codex_native_integration::AppServerObservation, String> {
    let socket = TestSocket::new(socket_name)
        .map_err(|error| format!("native fixture directory should create: {error}"))?;
    let listener = UnixListener::bind(socket.path())
        .map_err(|error| format!("native fixture socket should bind: {error}"))?;
    let server = tokio::spawn(async move {
        let (stream, _address) = listener
            .accept()
            .await
            .map_err(|error| format!("native fixture should accept: {error}"))?;
        let mut websocket = tokio_tungstenite::accept_async(stream)
            .await
            .map_err(|error| format!("native fixture should upgrade: {error}"))?;

        let initialize = next_json(&mut websocket).await?;
        let initialize_id = initialize
            .get("id")
            .cloned()
            .ok_or_else(|| "initialize request omitted its id".to_owned())?;
        websocket
            .send(Message::Text(
                serde_json::json!({
                    "id": initialize_id,
                    "result": {
                        "userAgent": "codex_app_server_daemon/1.2.3 (macOS; arm64)"
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .map_err(|error| format!("initialize response should send: {error}"))?;

        let initialized = next_json(&mut websocket).await?;
        if initialized.get("method").and_then(Value::as_str) != Some("initialized") {
            return Err("native fixture expected the initialized notification".to_owned());
        }
        let status_read = next_json(&mut websocket).await?;
        if status_read.get("method").and_then(Value::as_str) != Some("remoteControl/status/read") {
            return Err("native fixture expected the Remote Control status request".to_owned());
        }
        let status_read_id = status_read
            .get("id")
            .ok_or_else(|| "status request omitted its id".to_owned())?;
        websocket
            .send(Message::Text(
                remote_status_response_with_claims(status_read_id, remote_status)
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|error| format!("status response should send: {error}"))?;
        Ok::<(), String>(())
    });

    let observation = observe_app_server(
        socket.path(),
        Duration::from_secs(1),
        Duration::from_secs(1),
    )
    .await
    .map_err(|error| format!("native app-server should be observed: {error}"))?;
    server
        .await
        .map_err(|error| format!("native fixture task should join: {error}"))??;
    Ok(observation)
}

async fn next_json<S>(
    websocket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<Value, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let frame = websocket
        .next()
        .await
        .ok_or_else(|| "native fixture closed before a frame".to_owned())?
        .map_err(|error| format!("native fixture frame failed: {error}"))?;
    let Message::Text(text) = frame else {
        return Err("native fixture received a non-text frame".to_owned());
    };
    serde_json::from_str(&text).map_err(|error| format!("native fixture JSON failed: {error}"))
}

fn remote_status_response(request_id: &Value, status: &str) -> Value {
    serde_json::json!({
        "id": request_id,
        "result": remote_status(status),
    })
}

fn remote_status(status: &str) -> Value {
    serde_json::json!({
        "status": status,
        "serverName": "owner-mac",
        "installationId": "install_123",
        "environmentId": "env_123",
    })
}

fn remote_status_with_claims(
    status: &str,
    server_name: &str,
    environment_id: Option<&str>,
) -> Value {
    let mut remote_status = serde_json::Map::new();
    remote_status.insert("status".to_owned(), serde_json::json!(status));
    remote_status.insert("serverName".to_owned(), serde_json::json!(server_name));
    if let Some(environment_id) = environment_id {
        remote_status.insert(
            "environmentId".to_owned(),
            serde_json::json!(environment_id),
        );
    }
    Value::Object(remote_status)
}

fn remote_status_response_with_claims(request_id: &Value, remote_status: Value) -> Value {
    serde_json::json!({
        "id": request_id,
        "result": remote_status,
    })
}

struct TestSocket {
    directory: PathBuf,
    path: PathBuf,
}

impl TestSocket {
    fn new(name: &str) -> std::io::Result<Self> {
        let counter = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        // Use a short parent: Darwin TMPDIR plus the fixture name can exceed sockaddr_un.
        let directory = PathBuf::from("/tmp").join(format!(
            "codex-native-integration-{name}-{}-{counter}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("app-server.sock");
        Ok(Self { directory, path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestSocket {
    fn drop(&mut self) {
        let _socket_cleanup_result = std::fs::remove_file(&self.path);
        let _directory_cleanup_result = std::fs::remove_dir(&self.directory);
    }
}
