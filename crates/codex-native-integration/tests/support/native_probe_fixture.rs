use std::io;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_native_integration::CodexProtocolError;
use codex_native_integration::NativeObservationStage;
use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

pub(super) const FIXTURE_TIMEOUT: Duration = Duration::from_secs(2);
const FIXTURE_ACCEPT_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const REMOTE_WAIT: Duration = Duration::from_millis(100);
#[derive(Debug)]
pub(super) struct FixtureSocket {
    directory: PathBuf,
    path: PathBuf,
}

impl FixtureSocket {
    pub(super) fn new(name: &str) -> io::Result<Self> {
        let directory = PathBuf::from("/tmp").join(format!(
            "codex-native-probe-{name}-{}-{}",
            std::process::id(),
            FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("app-server.sock");
        Ok(Self { directory, path })
    }

    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for FixtureSocket {
    fn drop(&mut self) {
        let _socket_cleanup_result = std::fs::remove_file(&self.path);
        let _directory_cleanup_result = std::fs::remove_dir(&self.directory);
    }
}

static FIXTURE_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) struct FixturePlan {
    initialize: InitializeBehavior,
    action: ActionBehavior,
    reject_websocket_upgrade: bool,
}

impl FixturePlan {
    pub(super) fn respond_with(result: Value) -> Self {
        Self::with_action(ActionBehavior::Respond {
            result,
            changed: None,
            observed: None,
        })
    }

    pub(super) fn respond_then_change(result: Value, changed: Value) -> Self {
        Self::with_action(ActionBehavior::Respond {
            result,
            changed: Some(changed),
            observed: None,
        })
    }

    pub(super) fn respond_after(result: Value, delay: Duration) -> Self {
        Self::with_action(ActionBehavior::RespondAfter { result, delay })
    }

    pub(super) fn with_action(action: ActionBehavior) -> Self {
        Self {
            initialize: InitializeBehavior::Ready,
            action,
            reject_websocket_upgrade: false,
        }
    }

    pub(super) fn hold_action(observed: oneshot::Sender<()>) -> Self {
        Self::with_action(ActionBehavior::Hold {
            observed: Some(observed),
        })
    }

    pub(super) fn with_initialize(behavior: InitializeBehavior) -> Self {
        Self {
            initialize: behavior,
            action: ActionBehavior::Close,
            reject_websocket_upgrade: false,
        }
    }

    pub(super) fn reject_websocket_upgrade() -> Self {
        Self {
            initialize: InitializeBehavior::Ready,
            action: ActionBehavior::Close,
            reject_websocket_upgrade: true,
        }
    }
}

pub(super) enum InitializeBehavior {
    Ready,
    MalformedJson,
    MissingResult,
    Closed,
    Hold,
    InvalidUserAgent,
}

pub(super) enum ActionBehavior {
    Respond {
        result: Value,
        changed: Option<Value>,
        observed: Option<oneshot::Sender<()>>,
    },
    RespondAfter {
        result: Value,
        delay: Duration,
    },
    MissingResult,
    MalformedJson,
    Close,
    Hold {
        observed: Option<oneshot::Sender<()>>,
    },
}

pub(super) struct FixtureTranscript {
    pub(super) initialize_request: Option<Value>,
    pub(super) initialized_notification: Option<Value>,
    pub(super) action_request: Option<Value>,
    pub(super) peer_end: Option<PeerEnd>,
    pub(super) delayed_response_sent: bool,
}

#[derive(Debug)]
pub(super) enum PeerEnd {
    CloseFrame,
    Closed,
    UnexpectedMessage(String),
    TimedOut,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum FailureCase {
    WebSocket,
    Json,
    ClosedInitialize,
    InvalidInitializeResponse,
    InvalidUserAgent,
    NativeReadinessTimeout,
}

#[derive(Clone, Copy)]
pub(super) enum ExpectedFailure {
    WebSocket,
    Json,
    Closed(NativeObservationStage),
    InvalidResponse(NativeObservationStage),
    InvalidUserAgent,
    Timeout(NativeObservationStage),
}

impl FailureCase {
    pub(super) fn plan(self) -> FixturePlan {
        match self {
            Self::WebSocket => FixturePlan {
                initialize: InitializeBehavior::Ready,
                action: ActionBehavior::Close,
                reject_websocket_upgrade: true,
            },
            Self::Json => FixturePlan {
                initialize: InitializeBehavior::MalformedJson,
                action: ActionBehavior::Close,
                reject_websocket_upgrade: false,
            },
            Self::ClosedInitialize => FixturePlan {
                initialize: InitializeBehavior::Closed,
                action: ActionBehavior::Close,
                reject_websocket_upgrade: false,
            },
            Self::InvalidInitializeResponse => FixturePlan {
                initialize: InitializeBehavior::MissingResult,
                action: ActionBehavior::Close,
                reject_websocket_upgrade: false,
            },
            Self::InvalidUserAgent => FixturePlan {
                initialize: InitializeBehavior::InvalidUserAgent,
                action: ActionBehavior::Close,
                reject_websocket_upgrade: false,
            },
            Self::NativeReadinessTimeout => FixturePlan {
                initialize: InitializeBehavior::Hold,
                action: ActionBehavior::Close,
                reject_websocket_upgrade: false,
            },
        }
    }

    pub(super) fn expected(self) -> ExpectedFailure {
        match self {
            Self::WebSocket => ExpectedFailure::WebSocket,
            Self::Json => ExpectedFailure::Json,
            Self::ClosedInitialize => ExpectedFailure::Closed(NativeObservationStage::Initialize),
            Self::InvalidInitializeResponse => {
                ExpectedFailure::InvalidResponse(NativeObservationStage::Initialize)
            }
            Self::InvalidUserAgent => ExpectedFailure::InvalidUserAgent,
            Self::NativeReadinessTimeout => {
                ExpectedFailure::Timeout(NativeObservationStage::NativeReadiness)
            }
        }
    }
}

pub(super) fn matches_expected_failure(
    error: &CodexProtocolError,
    expected: ExpectedFailure,
) -> bool {
    match expected {
        ExpectedFailure::WebSocket => matches!(error, CodexProtocolError::WebSocket(_)),
        ExpectedFailure::Json => matches!(error, CodexProtocolError::Json(_)),
        ExpectedFailure::Closed(stage) => {
            matches!(error, CodexProtocolError::Closed { stage: found } if *found == stage)
        }
        ExpectedFailure::InvalidResponse(stage) => {
            matches!(error, CodexProtocolError::InvalidResponse { stage: found } if *found == stage)
        }
        ExpectedFailure::InvalidUserAgent => {
            matches!(error, CodexProtocolError::InvalidUserAgent)
        }
        ExpectedFailure::Timeout(stage) => {
            matches!(error, CodexProtocolError::Timeout { stage: found } if *found == stage)
        }
    }
}

pub(super) fn spawn_fixture(
    listener: UnixListener,
    plan: FixturePlan,
) -> JoinHandle<Result<FixtureTranscript, String>> {
    tokio::spawn(serve_fixture(listener, plan))
}

pub(super) async fn serve_fixture(
    listener: UnixListener,
    plan: FixturePlan,
) -> Result<FixtureTranscript, String> {
    let (mut stream, _address) = timeout(FIXTURE_ACCEPT_TIMEOUT, listener.accept())
        .await
        .map_err(|_| "native client did not connect to the fixture".to_owned())?
        .map_err(|error| format!("fixture accept failed: {error}"))?;
    let mut transcript = FixtureTranscript {
        initialize_request: None,
        initialized_notification: None,
        action_request: None,
        peer_end: None,
        delayed_response_sent: false,
    };

    if plan.reject_websocket_upgrade {
        stream
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
            )
            .await
            .map_err(|error| format!("fixture rejection response failed: {error}"))?;
        return Ok(transcript);
    }

    let mut websocket = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|error| format!("fixture websocket upgrade failed: {error}"))?;
    let initialize_request = next_json(&mut websocket).await?;
    transcript.initialize_request = Some(initialize_request.clone());
    let initialize_id = initialize_request
        .get("id")
        .cloned()
        .ok_or_else(|| "initialize request omitted its id".to_owned())?;

    match plan.initialize {
        InitializeBehavior::Ready => {
            send_json(
                &mut websocket,
                json!({
                    "id": initialize_id,
                    "result": {
                        "userAgent": "codex_app_server_daemon/1.2.3 fixture"
                    }
                }),
            )
            .await?;
        }
        InitializeBehavior::MalformedJson => {
            send_text(&mut websocket, "not-json").await?;
            transcript.peer_end = Some(wait_for_peer_end(&mut websocket).await);
            return Ok(transcript);
        }
        InitializeBehavior::MissingResult => {
            send_json(
                &mut websocket,
                json!({ "id": initialize_id, "error": { "code": -32000 } }),
            )
            .await?;
            transcript.peer_end = Some(wait_for_peer_end(&mut websocket).await);
            return Ok(transcript);
        }
        InitializeBehavior::Closed => {
            websocket
                .close(None)
                .await
                .map_err(|error| format!("fixture websocket close failed: {error}"))?;
            transcript.peer_end = Some(wait_for_peer_end(&mut websocket).await);
            return Ok(transcript);
        }
        InitializeBehavior::Hold => {
            transcript.peer_end = Some(wait_for_peer_end(&mut websocket).await);
            return Ok(transcript);
        }
        InitializeBehavior::InvalidUserAgent => {
            send_json(
                &mut websocket,
                json!({
                    "id": initialize_id,
                    "result": { "userAgent": "codex_app_server_daemon" }
                }),
            )
            .await?;
            transcript.peer_end = Some(wait_for_peer_end(&mut websocket).await);
            return Ok(transcript);
        }
    }

    let initialized_notification = next_json(&mut websocket).await?;
    transcript.initialized_notification = Some(initialized_notification);
    let action_request = next_json(&mut websocket).await?;
    transcript.action_request = Some(action_request.clone());
    let action_id = action_request
        .get("id")
        .cloned()
        .ok_or_else(|| "Remote Control request omitted its id".to_owned())?;

    match plan.action {
        ActionBehavior::Respond {
            result,
            changed,
            observed,
        } => {
            if let Some(observed) = observed {
                let _signal_result = observed.send(());
            }
            send_json(&mut websocket, json!({ "id": action_id, "result": result })).await?;
            if let Some(changed) = changed {
                send_json(
                    &mut websocket,
                    json!({
                        "method": "remoteControl/status/changed",
                        "params": changed,
                    }),
                )
                .await?;
            }
        }
        ActionBehavior::RespondAfter { result, delay } => {
            match timeout(delay, websocket.next()).await {
                Err(_) => {
                    send_json(&mut websocket, json!({ "id": action_id, "result": result })).await?;
                    transcript.delayed_response_sent = true;
                }
                Ok(None) | Ok(Some(Err(_))) => {
                    transcript.peer_end = Some(PeerEnd::Closed);
                    return Ok(transcript);
                }
                Ok(Some(Ok(Message::Close(_)))) => {
                    transcript.peer_end = Some(PeerEnd::CloseFrame);
                    return Ok(transcript);
                }
                Ok(Some(Ok(Message::Text(text)))) => {
                    transcript.peer_end = Some(PeerEnd::UnexpectedMessage(text.to_string()));
                    return Ok(transcript);
                }
                Ok(Some(Ok(message))) => {
                    transcript.peer_end = Some(PeerEnd::UnexpectedMessage(format!("{message:?}")));
                    return Ok(transcript);
                }
            }
        }
        ActionBehavior::MissingResult => {
            send_json(
                &mut websocket,
                json!({ "id": action_id, "error": { "code": -32000 } }),
            )
            .await?;
        }
        ActionBehavior::MalformedJson => send_text(&mut websocket, "not-json").await?,
        ActionBehavior::Close => {
            websocket
                .close(None)
                .await
                .map_err(|error| format!("fixture websocket close failed: {error}"))?;
            transcript.peer_end = Some(wait_for_peer_end(&mut websocket).await);
            return Ok(transcript);
        }
        ActionBehavior::Hold { observed } => {
            if let Some(observed) = observed {
                let _signal_result = observed.send(());
            }
        }
    }

    transcript.peer_end = Some(wait_for_peer_end(&mut websocket).await);
    Ok(transcript)
}

async fn send_json<S>(
    websocket: &mut tokio_tungstenite::WebSocketStream<S>,
    value: Value,
) -> Result<(), String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    websocket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|error| format!("fixture JSON frame failed: {error}"))
}

async fn send_text<S>(
    websocket: &mut tokio_tungstenite::WebSocketStream<S>,
    value: &str,
) -> Result<(), String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    websocket
        .send(Message::Text(value.to_owned().into()))
        .await
        .map_err(|error| format!("fixture text frame failed: {error}"))
}

async fn next_json<S>(
    websocket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<Value, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let frame = timeout(FIXTURE_TIMEOUT, websocket.next())
        .await
        .map_err(|_| "fixture timed out while waiting for a request".to_owned())?
        .ok_or_else(|| "native client closed before sending a request".to_owned())?
        .map_err(|error| format!("native request frame failed: {error}"))?;
    let Message::Text(text) = frame else {
        return Err("native client sent a non-text request frame".to_owned());
    };
    serde_json::from_str(&text).map_err(|error| format!("native request JSON failed: {error}"))
}

async fn wait_for_peer_end<S>(websocket: &mut tokio_tungstenite::WebSocketStream<S>) -> PeerEnd
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    match timeout(FIXTURE_TIMEOUT, websocket.next()).await {
        Err(_) => PeerEnd::TimedOut,
        Ok(None) | Ok(Some(Err(_))) => PeerEnd::Closed,
        Ok(Some(Ok(Message::Close(_)))) => {
            let _close_result = websocket.close(None).await;
            PeerEnd::CloseFrame
        }
        Ok(Some(Ok(Message::Text(text)))) => PeerEnd::UnexpectedMessage(text.to_string()),
        Ok(Some(Ok(message))) => PeerEnd::UnexpectedMessage(format!("{message:?}")),
    }
}

pub(super) fn expected_initialize_request() -> Value {
    json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "codex_router_host",
                "title": "Codex Router Host",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": { "experimentalApi": true },
        },
    })
}

pub(super) fn expected_status_request() -> Value {
    json!({ "id": 2, "method": "remoteControl/status/read" })
}

pub(super) fn expected_enable_request() -> Value {
    json!({
        "id": 2,
        "method": "remoteControl/enable",
        "params": { "ephemeral": true },
    })
}

pub(super) fn status(status: &str, server_name: &str, environment_id: Option<&str>) -> Value {
    let mut status_result = serde_json::Map::new();
    status_result.insert("status".to_owned(), json!(status));
    status_result.insert("serverName".to_owned(), json!(server_name));
    status_result.insert("installationId".to_owned(), json!("install_123"));
    if let Some(environment_id) = environment_id {
        status_result.insert("environmentId".to_owned(), json!(environment_id));
    }
    Value::Object(status_result)
}
