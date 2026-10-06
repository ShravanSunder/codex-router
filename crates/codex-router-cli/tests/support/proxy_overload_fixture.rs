use std::fs;
use std::net::TcpListener as StdTcpListener;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::openai_account_credential_bundle_key;
use codex_router_secret_store::test_support::open_encrypted_credential_store;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::sqlite::SqliteStateStore;
use futures_util::SinkExt;
use serde_json::Value;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::process::Child;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::Duration as TokioDuration;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::Request;
use tokio_tungstenite::tungstenite::handshake::server::Response;

const ROUTER_STARTUP_TIMEOUT: TokioDuration = TokioDuration::from_secs(10);
const ROUTER_EXIT_TIMEOUT: TokioDuration = TokioDuration::from_secs(10);
const UPSTREAM_ACCEPT_TIMEOUT: TokioDuration = TokioDuration::from_secs(5);
const MAX_ROUTER_STDERR_BYTES: usize = 16 * 1024;
const ROUTER_BINARY: &str = env!("CARGO_BIN_EXE_codex-router");
const FAKE_ACCOUNT_IDS: [&str; 2] = ["acct_proxy_overload_primary", "acct_proxy_overload_spare"];

pub(crate) type TestUpstreamWebSocket = WebSocketStream<TcpStream>;
pub(crate) type TestLocalWebSocket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpstreamHandshakeObservation {
    pub(crate) request_path: String,
    pub(crate) selected_account_id: Option<String>,
    pub(crate) thread_id_values: Vec<String>,
}

pub(crate) struct UpstreamSession {
    pub(crate) websocket: TestUpstreamWebSocket,
    pub(crate) handshake: UpstreamHandshakeObservation,
    explicit_response_create_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PersistedStateObservation {
    pub(crate) quota_exhaustion_state_rows: i64,
    pub(crate) quota_exhaustion_snapshot_rows: i64,
    pub(crate) eligible_selector_window_rows: i64,
    pub(crate) response_owner_rows_for_selected_account: i64,
}

pub(crate) struct RouterFixture {
    child: Child,
    stdout_task: JoinHandle<()>,
    stderr_task: JoinHandle<Vec<u8>>,
    upstream_listener: Option<TcpListener>,
    upstream_connection_count: usize,
    state_path: PathBuf,
    registry_report_path: PathBuf,
    router_port: u16,
    account_ids: [String; 2],
    _temporary_root: TempDir,
}

impl RouterFixture {
    pub(crate) async fn start(expected_websocket_sessions: usize) -> Result<Self, String> {
        if expected_websocket_sessions == 0 {
            return Err("Router fixture needs at least one WebSocket session".to_owned());
        }

        let temporary_root = tempfile::tempdir()
            .map_err(|error| format!("create isolated Router fixture root: {error}"))?;
        let state_path = temporary_root.path().join("router-state.sqlite");
        let secret_root = temporary_root.path().join("router-secrets");
        let registry_report_path = temporary_root.path().join("websocket-registry.json");
        let codex_home = temporary_root.path().join("codex-home");
        fs::create_dir_all(&codex_home)
            .map_err(|error| format!("create isolated Codex home: {error}"))?;

        let account_ids = FAKE_ACCOUNT_IDS.map(str::to_owned);
        seed_fixture_accounts(&state_path, &secret_root, &account_ids)?;

        let upstream_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("bind fake provider upstream: {error}"))?;
        let upstream_address = upstream_listener
            .local_addr()
            .map_err(|error| format!("read fake provider address: {error}"))?;
        let router_port = reserve_loopback_port()?;

        let mut child = Command::new(ROUTER_BINARY)
            .args(["serve", "--listen-host", "127.0.0.1", "--port"])
            .arg(router_port.to_string())
            .args(["--state-db"])
            .arg(&state_path)
            .args(["--secret-root"])
            .arg(&secret_root)
            .args(["--upstream-base-url"])
            .arg(format!("http://{upstream_address}/v1"))
            .args(["--disable-background-quota-refresh", "--max-connections"])
            .arg(expected_websocket_sessions.to_string())
            .args(["--websocket-registry-report-file"])
            .arg(&registry_report_path)
            .env("HOME", temporary_root.path())
            .env("CODEX_HOME", &codex_home)
            .env("OTEL_SDK_DISABLED", "true")
            .env_remove("CODEX_ROUTER_DEBUG_ROUTER_ROOT")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("spawn compiled Router test child: {error}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "compiled Router child stdout was not piped".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "compiled Router child stderr was not piped".to_owned())?;
        let stderr_task = tokio::spawn(async move { drain_bounded_stderr(stderr).await });
        let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
        let stdout_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let mut ready_sender = Some(ready_sender);
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        if let Some(ready_sender) = ready_sender.take() {
                            let _ = ready_sender.send(Err(
                                "Router child exited before announcing its loopback listener"
                                    .to_owned(),
                            ));
                        }
                        return;
                    }
                    Ok(_) if line.contains("listening: 127.0.0.1:") => {
                        if let Some(ready_sender) = ready_sender.take() {
                            let _ = ready_sender.send(Ok(()));
                        }
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        if let Some(ready_sender) = ready_sender.take() {
                            let _ = ready_sender
                                .send(Err(format!("read Router startup output: {error}")));
                        }
                        return;
                    }
                }
            }
            let _ = tokio::io::copy(&mut reader, &mut tokio::io::sink()).await;
        });

        let readiness_result = match timeout(ROUTER_STARTUP_TIMEOUT, ready_receiver).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err("Router startup readiness sender ended".to_owned()),
            Err(_) => Err("Router did not announce readiness within 10 seconds".to_owned()),
        };
        if let Err(readiness_error) = readiness_result {
            let _ = child.start_kill();
            let _ = timeout(ROUTER_EXIT_TIMEOUT, child.wait()).await;
            let _ = timeout(ROUTER_EXIT_TIMEOUT, stdout_task).await;
            let stderr = timeout(ROUTER_EXIT_TIMEOUT, stderr_task)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            return Err(format!(
                "{readiness_error}; Router stderr: {}",
                redact_fixture_credentials(&stderr)
            ));
        }

        Ok(Self {
            child,
            stdout_task,
            stderr_task,
            upstream_listener: Some(upstream_listener),
            upstream_connection_count: 0,
            state_path,
            registry_report_path,
            router_port,
            account_ids,
            _temporary_root: temporary_root,
        })
    }

    pub(crate) const fn router_port(&self) -> u16 {
        self.router_port
    }

    pub(crate) fn account_ids(&self) -> &[String; 2] {
        &self.account_ids
    }

    pub(crate) const fn upstream_connection_count(&self) -> usize {
        self.upstream_connection_count
    }

    pub(crate) fn close_upstream_listener(&mut self) {
        self.upstream_listener.take();
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn accept_upstream(&mut self) -> Result<UpstreamSession, String> {
        let listener = self
            .upstream_listener
            .as_ref()
            .ok_or_else(|| "fake upstream listener is already closed".to_owned())?;
        let (stream, _) = timeout(UPSTREAM_ACCEPT_TIMEOUT, listener.accept())
            .await
            .map_err(|_| "Router did not connect to the fake upstream within 5 seconds".to_owned())?
            .map_err(|error| format!("accept Router upstream connection: {error}"))?;
        let captured_handshake = Arc::new(Mutex::new(None));
        let callback_capture = Arc::clone(&captured_handshake);
        let websocket = timeout(
            UPSTREAM_ACCEPT_TIMEOUT,
            accept_hdr_async(stream, move |request: &Request, response: Response| {
                let observation = capture_upstream_handshake(request);
                if let Ok(mut captured) = callback_capture.lock() {
                    *captured = Some(observation);
                }
                Ok(response)
            }),
        )
        .await
        .map_err(|_| "complete fake upstream WebSocket handshake within 5 seconds".to_owned())?
        .map_err(|error| format!("accept Router upstream WebSocket handshake: {error}"))?;
        let handshake = captured_handshake
            .lock()
            .map_err(|_| "fake upstream handshake observation lock poisoned".to_owned())?
            .take()
            .ok_or_else(|| "fake upstream handshake was not observed".to_owned())?;
        self.upstream_connection_count += 1;

        Ok(UpstreamSession {
            websocket,
            handshake,
            explicit_response_create_count: 0,
        })
    }

    pub(crate) async fn has_unexpected_upstream_connection(
        &mut self,
        observation_window: TokioDuration,
    ) -> Result<bool, String> {
        let Some(listener) = self.upstream_listener.as_ref() else {
            return Ok(false);
        };
        match timeout(observation_window, listener.accept()).await {
            Ok(Ok((stream, _))) => {
                drop(stream);
                Ok(true)
            }
            Ok(Err(error)) => Err(format!("observe fake upstream listener: {error}")),
            Err(_) => Ok(false),
        }
    }

    pub(crate) async fn inspect_persisted_state(
        &self,
        selected_account_id: &str,
    ) -> Result<PersistedStateObservation, String> {
        let options = SqliteConnectOptions::new()
            .filename(&self.state_path)
            .read_only(true)
            .create_if_missing(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|error| format!("open fixture state read-only after Router exit: {error}"))?;

        let quota_exhaustion_state_rows = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM route_band_account_states
              WHERE route_band = 'responses' AND reason_code = 'provider_quota_exhausted'",
        )
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("read persisted quota-exhaustion state: {error}"))?;
        let quota_exhaustion_snapshot_rows = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM quota_snapshots
              WHERE account_id = ?1 AND route_band = 'responses'
                AND source = 'openai_endpoint' AND remaining_headroom = 0
                AND reset_unix_seconds IS NULL",
        )
        .bind(selected_account_id)
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("read persisted quota snapshot: {error}"))?;
        let eligible_selector_window_rows = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM selector_quota_windows
              WHERE route_band = 'responses' AND status = 'eligible'
                AND remaining_headroom = 100",
        )
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("read eligible fixture quota windows: {error}"))?;
        let response_owner_rows_for_selected_account = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM previous_response_affinity_owners
              WHERE account_id = ?1 AND route_band = 'responses'",
        )
        .bind(selected_account_id)
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("read same-account previous-response owner writes: {error}"))?;

        pool.close().await;
        Ok(PersistedStateObservation {
            quota_exhaustion_state_rows,
            quota_exhaustion_snapshot_rows,
            eligible_selector_window_rows,
            response_owner_rows_for_selected_account,
        })
    }

    pub(crate) async fn wait_for_exit(&mut self) -> Result<Value, String> {
        let exit_status = timeout(ROUTER_EXIT_TIMEOUT, self.child.wait())
            .await
            .map_err(|_| "Router child did not finish its bounded connection set".to_owned())?
            .map_err(|error| format!("wait for Router child exit: {error}"))?;
        timeout(ROUTER_EXIT_TIMEOUT, &mut self.stdout_task)
            .await
            .map_err(|_| "Router stdout drain did not finish after process exit".to_owned())?
            .map_err(|error| format!("join Router stdout drain: {error}"))?;
        let stderr = timeout(ROUTER_EXIT_TIMEOUT, &mut self.stderr_task)
            .await
            .map_err(|_| "Router stderr drain did not finish after process exit".to_owned())?
            .map_err(|error| format!("join Router stderr drain: {error}"))?;
        if !exit_status.success() {
            return Err(format!(
                "Router test child exited with {exit_status}; stderr: {}",
                redact_fixture_credentials(&stderr)
            ));
        }

        let report_text = fs::read_to_string(&self.registry_report_path)
            .map_err(|error| format!("read Router registry report: {error}"))?;
        serde_json::from_str(&report_text)
            .map_err(|error| format!("parse Router registry report: {error}"))
    }
}

impl UpstreamSession {
    pub(crate) async fn receive_client_frame(&mut self) -> Result<Message, String> {
        let message = timeout(
            UPSTREAM_ACCEPT_TIMEOUT,
            futures_util::StreamExt::next(&mut self.websocket),
        )
        .await
        .map_err(|_| "wait for explicit client frame at fake upstream".to_owned())?
        .ok_or_else(|| "Router upstream socket ended before the expected client frame".to_owned())?
        .map_err(|error| format!("read explicit client frame at fake upstream: {error}"))?;
        self.note_response_create(&message);
        Ok(message)
    }

    pub(crate) async fn send_provider_frame(
        &mut self,
        payload: &'static str,
    ) -> Result<(), String> {
        timeout(
            UPSTREAM_ACCEPT_TIMEOUT,
            self.websocket.send(Message::Text(payload.into())),
        )
        .await
        .map_err(|_| "send fake provider frame to Router within 5 seconds".to_owned())?
        .map_err(|error| format!("send fake provider frame to Router: {error}"))
    }

    pub(crate) async fn receive_unrequested_frame(
        &mut self,
        observation_window: Duration,
    ) -> Result<Option<Message>, String> {
        match timeout(
            observation_window,
            futures_util::StreamExt::next(&mut self.websocket),
        )
        .await
        {
            Ok(Some(Ok(message))) => {
                self.note_response_create(&message);
                Ok(Some(message))
            }
            Ok(Some(Err(error))) => Err(format!("read same-socket continuation probe: {error}")),
            Ok(None) => Ok(Some(Message::Close(None))),
            Err(_) => Ok(None),
        }
    }

    pub(crate) const fn explicit_response_create_count(&self) -> usize {
        self.explicit_response_create_count
    }

    fn note_response_create(&mut self, message: &Message) {
        let Message::Text(text) = message else {
            return;
        };
        let is_response_create = serde_json::from_str::<Value>(text.as_str())
            .ok()
            .and_then(|value| value.get("type").and_then(Value::as_str).map(str::to_owned))
            .is_some_and(|message_type| message_type == "response.create");
        if is_response_create {
            self.explicit_response_create_count += 1;
        }
    }
}

pub(crate) async fn connect_local_client(
    router_port: u16,
    thread_id_values: &[String],
) -> Result<WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, String> {
    let mut request = format!("ws://127.0.0.1:{router_port}/v1/responses")
        .into_client_request()
        .map_err(|error| format!("build local Router WebSocket request: {error}"))?;
    for thread_id in thread_id_values {
        let header_value =
            tokio_tungstenite::tungstenite::http::HeaderValue::from_bytes(thread_id.as_bytes())
                .map_err(|error| format!("build fixture thread-id header: {error}"))?;
        request.headers_mut().append("thread-id", header_value);
    }

    timeout(
        UPSTREAM_ACCEPT_TIMEOUT,
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| "connect explicit WebSocket client to Router within 5 seconds".to_owned())?
    .map(|(websocket, _)| websocket)
    .map_err(|error| format!("connect explicit WebSocket client to Router: {error}"))
}

pub(crate) async fn send_explicit_response_create(
    websocket: &mut WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    sequence: usize,
) -> Result<String, String> {
    let payload = format!(r#"{{"type":"response.create","input":"fixture-{sequence}"}}"#);
    timeout(
        UPSTREAM_ACCEPT_TIMEOUT,
        websocket.send(Message::Text(payload.clone().into())),
    )
    .await
    .map_err(|_| "send explicit client response.create within 5 seconds".to_owned())?
    .map_err(|error| format!("send explicit client response.create: {error}"))?;
    Ok(payload)
}

pub(crate) async fn receive_client_frame(
    websocket: &mut WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
) -> Result<Message, String> {
    timeout(
        UPSTREAM_ACCEPT_TIMEOUT,
        futures_util::StreamExt::next(websocket),
    )
    .await
    .map_err(|_| "wait for explicit client WebSocket frame within 5 seconds".to_owned())?
    .ok_or_else(|| "local Router WebSocket ended before the expected frame".to_owned())?
    .map_err(|error| format!("read explicit client WebSocket frame: {error}"))
}

fn capture_upstream_handshake(request: &Request) -> UpstreamHandshakeObservation {
    let thread_id_values = request
        .headers()
        .get_all("thread-id")
        .iter()
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .collect();
    let selected_account_id = request
        .headers()
        .get("chatgpt-account-id")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
    UpstreamHandshakeObservation {
        request_path: request.uri().path().to_owned(),
        selected_account_id,
        thread_id_values,
    }
}

fn seed_fixture_accounts(
    state_path: &Path,
    secret_root: &Path,
    account_ids: &[String; 2],
) -> Result<(), String> {
    let state = SqliteStateStore::open(state_path)
        .map_err(|error| format!("open isolated fixture state database: {error}"))?;
    let secrets = open_encrypted_credential_store(secret_root)
        .map_err(|error| format!("open isolated fixture secret store: {error}"))?;
    let now_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("read fixture clock: {error}"))?
        .as_secs();

    for (index, account_id_value) in account_ids.iter().enumerate() {
        let account_id = AccountId::new(account_id_value.clone())
            .map_err(|error| format!("validate fixture account identity: {error}"))?;
        let label = format!("proxy-overload-fixture-{index}");
        let account = AccountRecord::new(
            Provider::Openai,
            account_id.clone(),
            label,
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        AccountStateRepository::upsert_account(&state, &account)
            .map_err(|error| format!("seed enabled fixture account: {error}"))?;

        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(100)
        .with_reset_unix_seconds(now_unix_seconds.saturating_add(18_000))
        .with_effective(true)
        .with_observed_unix_seconds(now_unix_seconds);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(100)
        .with_reset_unix_seconds(now_unix_seconds.saturating_add(604_800))
        .with_observed_unix_seconds(now_unix_seconds);
        SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
            &state,
            &account_id,
            "responses",
            &[short_window, weekly_window],
            now_unix_seconds,
            now_unix_seconds.saturating_add(86_400),
        )
        .map_err(|error| format!("seed eligible fixture quota windows: {error}"))?;

        let bundle_key = openai_account_credential_bundle_key(&account_id, 1)
            .map_err(|error| format!("build fixture credential key: {error}"))?;
        let fake_access_token = format!("proxy-overload-fixture-token-{index}");
        let credential = AccountCredentialBundle::imported_codex_auth(fake_access_token, None)
            .with_expires_unix_seconds(now_unix_seconds.saturating_add(86_400))
            .with_chatgpt_account_id(account_id_value.clone())
            .to_secret_string()
            .map_err(|error| format!("serialize fixture credential bundle: {error}"))?;
        secrets
            .write_secret(&bundle_key, &credential)
            .map_err(|error| format!("write fixture credential bundle: {error}"))?;
    }

    Ok(())
}

fn reserve_loopback_port() -> Result<u16, String> {
    let listener = StdTcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("reserve isolated Router port: {error}"))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| format!("read isolated Router port: {error}"))
}

async fn drain_bounded_stderr(mut stderr: tokio::process::ChildStderr) -> Vec<u8> {
    let mut captured = Vec::with_capacity(MAX_ROUTER_STDERR_BYTES);
    let mut buffer = [0_u8; 2048];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(bytes_read) => {
                let remaining_capacity = MAX_ROUTER_STDERR_BYTES.saturating_sub(captured.len());
                let bounded_bytes = bytes_read.min(remaining_capacity);
                let Some(read_bytes) = buffer.get(..bounded_bytes) else {
                    break;
                };
                captured.extend_from_slice(read_bytes);
            }
        }
    }
    captured
}

fn redact_fixture_credentials(stderr: &[u8]) -> String {
    let mut rendered = String::from_utf8_lossy(stderr).into_owned();
    for account_index in 0..FAKE_ACCOUNT_IDS.len() {
        let fixture_token = format!("proxy-overload-fixture-token-{account_index}");
        rendered = rendered.replace(&fixture_token, "[fixture token redacted]");
    }
    rendered
}
