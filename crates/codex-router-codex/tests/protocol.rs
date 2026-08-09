use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_router_codex::RemoteControlObservation;
use codex_router_codex::observe_app_server;
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

struct TestSocket {
    directory: PathBuf,
    path: PathBuf,
}

impl TestSocket {
    fn new(name: &str) -> std::io::Result<Self> {
        let counter = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "codex-router-codex-{name}-{}-{counter}",
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
