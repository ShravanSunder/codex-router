#[cfg(not(feature = "keychain-test-support"))]
compile_error!(
    "Responses recovery acceptance requires keychain-test-support and isolated file credentials"
);

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::{
    AccountCredentialBundle, openai_account_credential_bundle_key,
};
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::local_router_token::LocalRouterTokenService;
use codex_router_secret_store::test_support::open_encrypted_credential_store;
use codex_router_state::account::{AccountRecord, AccountStatus};
use codex_router_state::credential_maintenance::CredentialMaintenanceState;
use codex_router_state::quota_snapshot::{
    PersistedQuotaSnapshot, PersistedSelectorQuotaWindow, QuotaSnapshotSource,
    SelectorQuotaWindowStatus,
};
use codex_router_state::sqlite::AsyncSqliteStateStore;
use futures_util::{SinkExt, StreamExt};
use sqlx::Connection;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

type TestResult<TOutput> = Result<TOutput, Box<dyn std::error::Error>>;

const LOCAL_TOKEN: &str = "synthetic-local-router-token";
const PRIMARY_TOKEN: &str = "synthetic-rejected-access";
const FALLBACK_TOKEN: &str = "synthetic-current-fallback-access";
const AUTH_REJECTION: &str = r#"{"error":{"code":"token_revoked","message":"Encountered invalidated oauth token for user, failing request"}}"#;
const HTTP_BODY: &[u8] =
    b"{\"model\":\"fixture-model\",\"input\":\"complete original body\"} \t\r\n ";
const FIRST_FRAME: &str =
    r#"{"type":"response.create","model":"fixture-model","input":"unchanged initial frame"}"#;
const COMPLETED_RESPONSE: &str =
    r#"{"type":"response.completed","response":{"id":"resp_fixture_complete"}}"#;

fn require_observation(condition: bool, message: &str) -> TestResult<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn assert_upstream_credentials(
    request: &tokio_tungstenite::tungstenite::handshake::server::Request,
    token: &str,
    account_header: &str,
) {
    assert_eq!(
        request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some(format!("Bearer {token}").as_str())
    );
    assert_eq!(
        request
            .headers()
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok()),
        Some(account_header)
    );
    assert!(!request.headers().contains_key("x-codex-router-token"));
}

async fn seed_private_root(root: &Path) -> TestResult<u64> {
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
    let state = AsyncSqliteStateStore::open(&root.join("state.sqlite")).await?;
    let secret_root = root.join("secrets");
    let secrets = open_encrypted_credential_store(&secret_root)?;
    let last_success = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("state.sqlite"));
    let mut seed_connection = sqlx::SqliteConnection::connect_with(&options).await?;
    LocalRouterTokenService::new(FileSecretStore::open(&secret_root)?)
        .rotate_with_token(LOCAL_TOKEN)?;
    for (name, generation, headroom, token, account_header) in [
        (
            "fixture-primary",
            1,
            90,
            PRIMARY_TOKEN,
            "fixture-primary-header",
        ),
        (
            "fixture-fallback",
            2,
            80,
            FALLBACK_TOKEN,
            "fixture-fallback-header",
        ),
    ] {
        let account_id = AccountId::new(name)?;
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Openai,
                    account_id.clone(),
                    name,
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(generation),
            )
            .await?;
        state
            .upsert_quota_snapshot(
                &PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                    .with_observed_unix_seconds(1_000)
                    .with_route_band("responses", headroom),
            )
            .await?;
        for (window_seconds, effective) in [(18_000, true), (604_800, false)] {
            state
                .upsert_selector_quota_window(
                    &PersistedSelectorQuotaWindow::new(
                        account_id.clone(),
                        "responses",
                        window_seconds,
                        SelectorQuotaWindowStatus::Eligible,
                    )
                    .with_remaining_headroom(headroom)
                    .with_effective(effective)
                    .with_observed_unix_seconds(1_000)
                    .with_reset_unix_seconds(1_000 + window_seconds),
                )
                .await?;
        }
        // Current file credentials include their last-success metadata so the
        // unchanged compiled CLI upkeep loop has no proactive renewal due.
        sqlx::query("INSERT INTO credential_maintenance (account_id, credential_generation, state, last_success_unix_seconds, consecutive_failures) VALUES (?1, ?2, ?3, ?4, 0)")
            .bind(name).bind(i64::try_from(generation)?).bind(CredentialMaintenanceState::Healthy.as_str()).bind(i64::try_from(last_success)?)
            .execute(&mut seed_connection).await?;
        let bundle = AccountCredentialBundle::imported_codex_auth(token, None)
            .with_expires_unix_seconds(4_000_000_000)
            .with_chatgpt_account_id(account_header);
        secrets.write_secret(
            &openai_account_credential_bundle_key(&account_id, generation)?,
            &bundle.to_secret_string()?,
        )?;
        assert!(
            !std::fs::read(secret_root.join(format!(
                "{}.v2",
                openai_account_credential_bundle_key(&account_id, generation)?.as_str()
            )))?
            .windows(token.len())
            .any(|window| window == token.as_bytes())
        );
    }
    seed_connection.close().await?;
    state.close().await?;
    Ok(last_success)
}

async fn start_compiled_serve(
    root: &Path,
    upstream: SocketAddr,
) -> TestResult<(Child, SocketAddr)> {
    // Match the existing CLI serve tests: profile-port validation rejects zero,
    // so reserve an available loopback port and pass the allocated value.
    let reservation = TcpListener::bind("127.0.0.1:0").await?;
    let router_port = reservation.local_addr()?.port().to_string();
    drop(reservation);
    let mut child = Command::new(env!("CARGO_BIN_EXE_codex-router"))
        .args([
            "serve",
            "--listen-host",
            "127.0.0.1",
            "--port",
            router_port.as_str(),
            "--state-db",
        ])
        .arg(root.join("state.sqlite"))
        .arg("--secret-root")
        .arg(root.join("secrets"))
        .args([
            "--upstream-base-url",
            &format!("http://{upstream}/v1"),
            "--now-unix-seconds",
            "1030",
            "--max-snapshot-age-seconds",
            "60",
            "--require-local-token",
            "--disable-background-quota-refresh",
            "--max-connections",
            "1",
            "--websocket-registry-report-file",
        ])
        .arg(root.join("registry.json"))
        .env("OTEL_SDK_DISABLED", "true")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT")
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .env_remove("CODEX_ROUTER_DEBUG_ROUTER_ROOT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().ok_or("child stdout missing")?;
    let readiness = tokio::time::timeout(Duration::from_secs(10), async {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(address) = line.strip_prefix("listening: ") {
                return Ok::<SocketAddr, Box<dyn std::error::Error>>(address.parse()?);
            }
        }
        Err("compiled serve exited before readiness".into())
    })
    .await;
    let address = match readiness {
        Ok(Ok(address)) => address,
        Ok(Err(error)) => {
            let output = child.wait_with_output().await?;
            return Err(format!(
                "{error}; exit={}; stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Err(error) => {
            child.kill().await?;
            child.wait().await?;
            return Err(error.into());
        }
    };
    assert!(address.ip().is_loopback());
    assert_ne!(address.port(), 0);
    Ok((child, address))
}

async fn finish_serve_and_check_state(
    child: Child,
    root: &Path,
    last_success: u64,
) -> TestResult<()> {
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await??;
    assert!(
        output.status.success(),
        "compiled serve failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("registry.json"))?)?;
    require_observation(
        report
            .get("handled_connections")
            .and_then(serde_json::Value::as_u64)
            == Some(1),
        "compiled serve did not handle exactly one connection",
    )?;
    require_observation(
        report
            .pointer("/websocket_registry/active_sessions")
            .and_then(serde_json::Value::as_u64)
            == Some(0),
        "compiled serve leaked a websocket registration",
    )?;
    let state = AsyncSqliteStateStore::open(&root.join("state.sqlite")).await?;
    for (name, generation) in [("fixture-primary", 1), ("fixture-fallback", 2)] {
        let account_id = AccountId::new(name)?;
        let account = state
            .load_account(&account_id)
            .await?
            .ok_or("fixture account missing")?;
        assert_eq!(account.status(), AccountStatus::Enabled);
        assert_eq!(account.active_credential_generation(), Some(generation));
        let maintenance = state
            .load_credential_maintenance(&account_id)
            .await?
            .ok_or("current fixture maintenance missing")?;
        assert_eq!(maintenance.credential_generation, generation);
        assert_eq!(maintenance.state, CredentialMaintenanceState::Healthy);
        assert_eq!(maintenance.last_success_unix_seconds, Some(last_success));
        assert_eq!(maintenance.failure_class, None);
        assert_eq!(maintenance.claim_purpose, None);
        assert_eq!(maintenance.claimed_successor_generation, None);
        assert_eq!(maintenance.claim_started_unix_seconds, None);
        assert_eq!(maintenance.claim_prior_state, None);
        assert_eq!(maintenance.next_attempt_unix_seconds, None);
        assert_eq!(maintenance.consecutive_failures, 0);
    }
    state.close().await?;
    Ok(())
}

async fn read_provider_http_request(stream: &mut TcpStream) -> TestResult<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buffer = [0; 1024];
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Err("provider request ended before headers".into());
        }
        bytes.extend_from_slice(buffer.get(..read).ok_or("read exceeded buffer")?);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8(bytes.get(..header_end).ok_or("headers missing")?.to_vec())?;
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>())
        })
        .ok_or("provider Content-Length missing")??;
    while bytes.len() < header_end + length {
        let mut buffer = [0; 1024];
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Err("provider request ended before complete body".into());
        }
        bytes.extend_from_slice(buffer.get(..read).ok_or("read exceeded buffer")?);
    }
    assert_eq!(length, HTTP_BODY.len());
    Ok((
        headers,
        bytes
            .get(header_end..header_end + length)
            .ok_or("body range missing")?
            .to_vec(),
    ))
}

#[tokio::test]
async fn compiled_serve_recovers_responses_401_using_complete_body_and_current_file_credentials()
-> TestResult<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        let root = tempfile::Builder::new().prefix("responses-http-recovery-").tempdir()?;
        let last_success = seed_private_root(root.path()).await?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let upstream = listener.local_addr()?;
        let provider = async {
            for (token, account_header, status, response_body) in [
                (PRIMARY_TOKEN, "fixture-primary-header", 401, AUTH_REJECTION),
                (FALLBACK_TOKEN, "fixture-fallback-header", 200, "compiled-recovery-success"),
            ] {
                let (mut stream, _) = listener.accept().await?;
                let (headers, body) = read_provider_http_request(&mut stream).await?;
                require_observation(body == HTTP_BODY, "compiled HTTP retry changed the complete body bytes")?;
                require_observation(headers.lines().any(|line| line == format!("authorization: Bearer {token}")), "wrong current credential at HTTP provider")?;
                require_observation(headers.lines().any(|line| line == format!("chatgpt-account-id: {account_header}")), "HTTP token and companion account header mismatch")?;
                require_observation(!headers.to_ascii_lowercase().contains("x-codex-router-token"), "local token forwarded to provider")?;
                stream.write_all(format!("HTTP/1.1 {status} fixture\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{response_body}", response_body.len()).as_bytes()).await?;
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        let (child, address) = start_compiled_serve(root.path(), upstream).await?;
        let client = async {
            let mut stream = TcpStream::connect(address).await?;
            stream.write_all(format!("POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nX-Codex-Router-Token: {LOCAL_TOKEN}\r\nContent-Length: {}\r\n\r\n", HTTP_BODY.len()).as_bytes()).await?;
            stream.write_all(HTTP_BODY).await?;
            stream.shutdown().await?;
            let mut response = String::new();
            stream.read_to_string(&mut response).await?;
            require_observation(response.starts_with("HTTP/1.1 200"), "HTTP auth failover did not return 200")?;
            require_observation(response.ends_with("compiled-recovery-success"), "HTTP fallback response missing")?;
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        tokio::try_join!(provider, client)?;
        finish_serve_and_check_state(child, root.path(), last_success).await?;
        let _released_listener = TcpListener::bind(address).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }).await?
}

#[tokio::test]
#[allow(clippy::result_large_err)] // tungstenite requires this unboxed callback error.
async fn compiled_serve_recovers_websocket_handshake_401_without_repeating_first_frame()
-> TestResult<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        let root = tempfile::Builder::new().prefix("responses-websocket-recovery-").tempdir()?;
        let last_success = seed_private_root(root.path()).await?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let upstream = listener.local_addr()?;
        let provider = async {
            let (stream, _) = listener.accept().await?;
            let rejection = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, _response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                assert_upstream_credentials(request, PRIMARY_TOKEN, "fixture-primary-header");
                Err(tokio_tungstenite::tungstenite::http::Response::builder().status(401).body(Some(AUTH_REJECTION.to_owned())).expect("fixture handshake rejection"))
            }).await;
            require_observation(matches!(rejection, Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status().as_u16() == 401), "provider handshake was not a literal 401 rejection")?;
            let (stream, _) = listener.accept().await?;
            let mut websocket = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                assert_upstream_credentials(request, FALLBACK_TOKEN, "fixture-fallback-header");
                Ok(response)
            }).await?;
            require_observation(websocket.next().await.ok_or("initial frame missing")??.to_text()? == FIRST_FRAME, "compiled WS retry changed the original frame")?;
            websocket.send(Message::text(COMPLETED_RESPONSE)).await?;
            while let Some(message) = websocket.next().await {
                let message = message?;
                require_observation(!matches!(message, Message::Text(_) | Message::Binary(_)), "original first frame repeated")?;
                if matches!(message, Message::Close(_)) { break; }
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        let (child, address) = start_compiled_serve(root.path(), upstream).await?;
        let client = async {
            let mut request = format!("ws://{address}/v1/responses").into_client_request()?;
            request.headers_mut().insert("x-codex-router-token", LOCAL_TOKEN.parse()?);
            let (mut websocket, response) = tokio_tungstenite::connect_async(request).await?;
            require_observation(response.status().as_u16() == 101, "local websocket did not upgrade")?;
            websocket.send(Message::text(FIRST_FRAME)).await?;
            require_observation(websocket.next().await.ok_or("completion missing")??.to_text()? == COMPLETED_RESPONSE, "same local websocket lost completion")?;
            websocket.close(None).await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        tokio::try_join!(provider, client)?;
        finish_serve_and_check_state(child, root.path(), last_success).await?;
        let _released_listener = TcpListener::bind(address).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }).await?
}
