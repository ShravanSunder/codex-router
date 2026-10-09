#![allow(unused_imports)]
use super::*;
pub(super) use std::env;
pub(super) use std::path::PathBuf;
pub(super) use std::sync::Arc;
pub(super) use std::sync::Mutex;
pub(super) use std::sync::MutexGuard;
pub(super) use std::sync::atomic::AtomicBool;
pub(super) use std::sync::atomic::AtomicUsize;
pub(super) use std::sync::atomic::Ordering;
pub(super) use std::sync::mpsc;
pub(super) use std::time::Duration;

pub(super) use codex_router_auth::resolver::CredentialRefreshClient;
pub(super) use codex_router_auth::resolver::CredentialRefreshFailure;
pub(super) use codex_router_core::ids::AccountId;
pub(super) use codex_router_core::redaction::SecretString;
pub(super) use codex_router_secret_store::SecretStore;
pub(super) use codex_router_secret_store::account_tokens::AccountCredentialBundle;
pub(super) use codex_router_secret_store::account_tokens::openai_account_credential_bundle_key;
pub(super) use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
pub(super) use codex_router_secret_store::model::SecretKey;
pub(super) use codex_router_secret_store::model::SecretStoreError;
pub(super) use codex_router_state::credential_maintenance::CredentialMaintenanceState;
pub(super) use codex_router_state::repositories::AccountStateRepository;
pub(super) use codex_router_state::sqlite::SqliteStateStore;
pub(super) use http_body_util::BodyExt;
pub(super) use http_body_util::StreamBody;
pub(super) use hyper::body::Frame;
pub(super) use tokio::io::AsyncWriteExt;

pub(super) static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) fn local_router_token(token: &str, generation: u64) -> LocalRouterTokenRecord {
    LocalRouterTokenRecord::new(
        SecretString::new(token),
        codex_router_core::ids::TokenGeneration::new(generation),
    )
}

pub(super) fn send_loopback_http_request(
    address: SocketAddr,
    path: &str,
    authorization: Option<&str>,
) -> String {
    use std::io::Read as _;
    use std::io::Write as _;
    use std::net::TcpStream;
    use std::time::Duration;

    let mut stream = TcpStream::connect(address)
        .unwrap_or_else(|error| panic!("loopback client should connect: {error}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap_or_else(|error| panic!("loopback read timeout should set: {error}"));
    let authorization_header =
        authorization.map_or(String::new(), |value| format!("Authorization: {value}\r\n"));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 2\r\n{authorization_header}\r\n{{}}"
    );
    stream
        .write_all(request.as_bytes())
        .unwrap_or_else(|error| panic!("loopback request should write: {error}"));
    stream
        .shutdown(std::net::Shutdown::Write)
        .unwrap_or_else(|error| panic!("loopback client write side should close: {error}"));

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .unwrap_or_else(|error| panic!("loopback response should read: {error}"));
    response
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn claude_edge_local_token_is_scoped_and_reloaded_without_enabling_codex_auth() {
    use codex_router_core::local_auth::LocalRouterAuth;
    use std::io::ErrorKind;
    use std::net::TcpListener;

    let upstream_listener =
        TcpListener::bind("127.0.0.1:0").expect("test upstream listener should bind");
    upstream_listener
        .set_nonblocking(true)
        .expect("test upstream listener should be nonblocking");
    let upstream_address = upstream_listener
        .local_addr()
        .expect("test upstream listener address should be available");
    let bind_address =
        LoopbackBindAddress::new("127.0.0.1", 0).expect("router loopback address should validate");
    let database_path = test_database_path("claude_local_token_admission");
    let secret_root = database_path.with_extension("secrets");
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        bind_address,
        UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
            .expect("test upstream endpoint should validate"),
        database_path,
        secret_root,
    )
    .with_claude_edge_local_token(
        local_router_token("initial-token", 1),
        Duration::from_secs(400),
    );
    let runtime = LoopbackRouterRuntime::start_for_test(config)
        .await
        .expect("router runtime should start for token-scope test");
    let router_address = runtime.local_addr();
    let local_auth_reloader = runtime.local_auth_reloader();
    let server_thread = tokio::spawn(async move { runtime.serve_http_connections(6).await });

    let unsupported_claude =
        send_loopback_http_request(router_address, "/anthropic/v1/messages/count_tokens", None);
    let unsupported_claude_body = unsupported_claude
        .split_once("\r\n\r\n")
        .map_or("", |(_headers, body)| body);
    assert!(
        unsupported_claude.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "unsupported Claude paths should return 404 without authentication: {unsupported_claude}"
    );
    assert!(
        unsupported_claude_body.contains("/anthropic/v1/messages/count_tokens")
            && unsupported_claude_body.contains("unsupported by Router"),
        "unsupported Claude response should name its path: {unsupported_claude_body}"
    );

    let unauthenticated_claude =
        send_loopback_http_request(router_address, "/anthropic/v1/messages", None);
    assert!(
        unauthenticated_claude.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "missing Claude token should reject before selection: {unauthenticated_claude}"
    );

    let unauthenticated_codex = send_loopback_http_request(router_address, "/v1/responses", None);
    assert!(
        unauthenticated_codex.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "Codex with optional local auth should reach its no-account result: {unauthenticated_codex}"
    );

    local_auth_reloader.reload_auth(LocalRouterAuth::new(
        local_router_token("reloaded-token", 2),
        Vec::new(),
    ));

    let stale_claude = send_loopback_http_request(
        router_address,
        "/anthropic/v1/messages",
        Some("Bearer initial-token"),
    );
    assert!(
        stale_claude.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "the replaced Claude token should reject: {stale_claude}"
    );

    let reloaded_claude = send_loopback_http_request(
        router_address,
        "/anthropic/v1/messages",
        Some("Bearer reloaded-token"),
    );
    assert!(
        reloaded_claude.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "the reloaded Claude token should pass admission and reach selection: {reloaded_claude}"
    );

    let codex_after_reload = send_loopback_http_request(router_address, "/v1/responses", None);
    assert!(
        codex_after_reload.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "token watcher reload must leave optional Codex auth disabled: {codex_after_reload}"
    );

    assert_eq!(
        server_thread
            .await
            .unwrap_or_else(|error| panic!("router server thread should join: {error:?}"))
            .expect("router should serve all six test requests"),
        6
    );
    match upstream_listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
        Ok((_stream, _peer)) => panic!("no test request should reach upstream"),
        Err(error) => panic!("upstream listener check should succeed: {error}"),
    }
}

#[derive(Clone)]
pub(super) struct HeldProxyRefreshClient {
    pub(super) entered_sender: mpsc::Sender<()>,
    pub(super) release_receiver: Arc<Mutex<mpsc::Receiver<()>>>,
    pub(super) completed_sender: mpsc::Sender<()>,
}

impl CredentialRefreshClient for HeldProxyRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialRefreshFailure> {
        self.entered_sender
            .send(())
            .expect("provider entry should report");
        self.release_receiver
            .lock()
            .expect("release lock")
            .recv_timeout(Duration::from_secs(5))
            .expect("provider should be released");
        self.completed_sender
            .send(())
            .expect("provider completion should report");
        Ok(AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000))
    }
}

pub(super) async fn proxy_refresh_fixture(
    case: &str,
    drain_limit: Duration,
) -> (
    LoopbackRouterRuntime,
    AccountId,
    PathBuf,
    EncryptedCredentialStore,
) {
    let database_path = test_database_path(case);
    let secret_root = database_path.with_extension("secrets");
    let account_id = AccountId::new("proxy-shutdown-account").expect("account id");
    let state = SqliteStateStore::open(&database_path).expect("fixture state");
    AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "shutdown",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1),
    )
    .expect("fixture account");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root)
            .expect("fixture secrets");
    let active_key = openai_account_credential_bundle_key(&account_id, 1).expect("active key");
    let active_bundle = AccountCredentialBundle::imported_codex_auth(
        "expired-access-canary",
        Some("old-refresh-canary".to_owned()),
    )
    .with_expires_unix_seconds(900)
    .to_secret_string()
    .expect("active bundle");
    secrets
        .write_secret(&active_key, &active_bundle)
        .expect("active secret");
    drop(state);
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new("127.0.0.1", 0).expect("loopback bind"),
        UpstreamEndpoint::new("http://127.0.0.1:1/v1").expect("fixture upstream"),
        database_path.clone(),
        secret_root,
    )
    .with_quota_clock(1_000, 300);
    let router = LoopbackRouterRuntime::start(config, secrets.clone())
        .await
        .expect("fixture router should start")
        .with_credential_refresh_shutdown_drain(drain_limit);
    (router, account_id, database_path, secrets)
}
