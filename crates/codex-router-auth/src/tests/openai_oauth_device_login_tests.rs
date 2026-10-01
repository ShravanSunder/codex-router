use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::openai_oauth::OpenAiOAuthDeviceLoginClient;
use crate::openai_oauth::OpenAiOAuthDeviceLoginError;

#[path = "openai_oauth_device_login_storage_tests.rs"]
mod storage_tests;

static TEMP_DIRECTORY_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[tokio::test]
async fn device_login_matches_upstream_requests_and_exchanges_pkce_verifier() {
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":"device-auth-id","usercode":"ABCD-EFGH","interval":"0"}"#,
        ),
        FakeResponse::json(403, "{}"),
        FakeResponse::json(404, "{}"),
        FakeResponse::json(
            200,
            r#"{"authorization_code":"authorization-code","code_challenge":"challenge","code_verifier":"verifier"}"#,
        ),
        FakeResponse::json(
            200,
            format!(
                r#"{{"id_token":"{}","access_token":"access-token-canary","refresh_token":"refresh-token-canary"}}"#,
                real_shaped_id_token(Some("chatgpt-account-id"))
            ),
        ),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url(),
        std::time::Duration::from_secs(60),
    );
    let cancellation = CancellationToken::new();

    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");
    assert_eq!(device_code.user_code(), "ABCD-EFGH");
    assert_eq!(
        device_code.verification_url(),
        format!("{}/codex/device", issuer.base_url())
    );
    let credentials = client
        .complete_device_code_login(&device_code, &cancellation)
        .await
        .expect("approved device login should exchange its verifier");

    assert_eq!(
        credentials.access_token().expose_secret(),
        "access-token-canary"
    );
    assert_eq!(
        credentials.refresh_token().expose_secret(),
        "refresh-token-canary"
    );
    assert_eq!(
        credentials
            .chatgpt_account_id()
            .map(|account| account.as_str()),
        Some("chatgpt-account-id")
    );

    let requests = issuer.finish().await;
    assert_eq!(requests.len(), 5);
    assert!(requests[0].starts_with("POST /api/accounts/deviceauth/usercode HTTP/1.1"));
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    assert!(requests[0].contains(r#"{"client_id":"app_EMoamEEZ73f0CkXaXp7hrann"}"#));
    for request in &requests[1..4] {
        assert!(request.starts_with("POST /api/accounts/deviceauth/token HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("content-type: application/json")
        );
        assert!(request.contains(r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH"}"#));
    }
    assert!(requests[4].starts_with("POST /oauth/token HTTP/1.1"));
    assert!(
        requests[4]
            .to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded")
    );
    for expected in [
        "grant_type=authorization_code",
        "client_id=app_EMoamEEZ73f0CkXaXp7hrann",
        "code=authorization-code",
        "code_verifier=verifier",
        "redirect_uri=http%3A%2F%2F",
        "%2Fdeviceauth%2Fcallback",
    ] {
        assert!(requests[4].contains(expected), "missing {expected}");
    }
}

#[tokio::test]
async fn device_code_poll_interval_defaults_to_five_seconds_and_honors_positive_values() {
    for (response_body, expected_interval) in [
        (
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH"}"#,
            std::time::Duration::from_secs(5),
        ),
        (
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"0"}"#,
            std::time::Duration::from_secs(5),
        ),
        (
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"7"}"#,
            std::time::Duration::from_secs(7),
        ),
    ] {
        let issuer = FakeDeviceIssuer::spawn(vec![FakeResponse::json(200, response_body)]).await;
        let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
            issuer.base_url(),
            std::time::Duration::from_secs(60),
        );
        let cancellation = CancellationToken::new();

        let device_code = client
            .request_user_code(&cancellation)
            .await
            .expect("user-code response should succeed");

        assert_eq!(device_code.poll_interval, expected_interval);
        assert_eq!(issuer.finish().await.len(), 1);
    }
}

#[tokio::test]
async fn stalled_device_code_poll_maps_timeout_to_poll_transport_failure() {
    let (poll_received_sender, poll_received) = oneshot::channel();
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"1"}"#,
        ),
        FakeResponse::stall_and_notify(poll_received_sender),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer_and_request_timeout(
        issuer.base_url(),
        std::time::Duration::from_secs(60),
        std::time::Duration::from_millis(100),
    );
    let cancellation = CancellationToken::new();
    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");
    let complete = client.complete_device_code_login(&device_code, &cancellation);
    tokio::pin!(complete);
    let error = tokio::select! {
        result = &mut complete => panic!("stalled poll completed before its timeout: {result:?}"),
        result = poll_received => {
            result.expect("fake issuer should receive the stalled poll");
            tokio::time::timeout(std::time::Duration::from_secs(2), &mut complete)
                .await
                .expect("the HTTP client should time out a stalled request")
                .expect_err("a timed-out poll should fail login")
        }
    };

    assert!(matches!(
        error,
        OpenAiOAuthDeviceLoginError::PollTransportFailure
    ));
    assert_eq!(issuer.finish().await.len(), 2);
}

#[tokio::test]
async fn cancellation_interrupts_a_stalled_device_code_poll_before_request_timeout() {
    let (poll_received_sender, poll_received) = oneshot::channel();
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"60"}"#,
        ),
        FakeResponse::stall_and_notify(poll_received_sender),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url(),
        std::time::Duration::from_secs(900),
    );
    let cancellation = CancellationToken::new();
    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");
    let complete = client.complete_device_code_login(&device_code, &cancellation);
    tokio::pin!(complete);

    let error = tokio::select! {
        result = &mut complete => panic!("stalled poll completed before cancellation: {result:?}"),
        result = poll_received => {
            result.expect("fake issuer should receive the stalled poll");
            cancellation.cancel();
            tokio::time::timeout(std::time::Duration::from_millis(250), &mut complete)
                .await
                .expect("cancellation should interrupt the request before its timeout")
                .expect_err("cancelled poll should stop login")
        }
    };

    assert!(matches!(error, OpenAiOAuthDeviceLoginError::Cancelled));
    assert_eq!(issuer.finish().await.len(), 2);
}

#[tokio::test]
async fn device_login_preserves_issuer_codes_and_trims_exchanged_tokens() {
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":" device-auth-id ","user_code":" ABCD-EFGH ","interval":"0"}"#,
        ),
        FakeResponse::json(
            200,
            r#"{"authorization_code":"authorization-code","code_challenge":"challenge","code_verifier":"verifier"}"#,
        ),
        FakeResponse::json(
            200,
            format!(
                r#"{{"id_token":"{}","access_token":" access-token-canary ","refresh_token":" refresh-token-canary "}}"#,
                real_shaped_id_token(None)
            ),
        ),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url(),
        std::time::Duration::from_secs(60),
    );
    let cancellation = CancellationToken::new();
    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");

    assert_eq!(device_code.user_code(), " ABCD-EFGH ");
    let credentials = client
        .complete_device_code_login(&device_code, &cancellation)
        .await
        .expect("approval should exchange trimmed token values");

    assert_eq!(
        credentials.access_token().expose_secret(),
        "access-token-canary"
    );
    assert_eq!(
        credentials.refresh_token().expose_secret(),
        "refresh-token-canary"
    );
    let requests = issuer.finish().await;
    assert!(
        requests[1].contains(r#"{"device_auth_id":" device-auth-id ","user_code":" ABCD-EFGH "}"#)
    );
}

#[tokio::test]
async fn empty_authorization_or_pkce_fields_reject_the_poll_response() {
    for response_body in [
        r#"{"authorization_code":"","code_challenge":"challenge","code_verifier":"verifier"}"#,
        r#"{"authorization_code":"authorization-code","code_challenge":"","code_verifier":"verifier"}"#,
        r#"{"authorization_code":"authorization-code","code_challenge":"challenge","code_verifier":""}"#,
    ] {
        let issuer = FakeDeviceIssuer::spawn(vec![
            FakeResponse::json(
                200,
                r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"0"}"#,
            ),
            FakeResponse::json(200, response_body),
        ])
        .await;
        let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
            issuer.base_url(),
            std::time::Duration::from_secs(60),
        );
        let cancellation = CancellationToken::new();
        let device_code = client
            .request_user_code(&cancellation)
            .await
            .expect("user-code request should succeed");

        let error = client
            .complete_device_code_login(&device_code, &cancellation)
            .await
            .expect_err("empty authorization and PKCE values must fail closed");

        assert!(matches!(
            error,
            OpenAiOAuthDeviceLoginError::InvalidPollResponse
        ));
        assert_eq!(issuer.finish().await.len(), 2);
    }
}

#[tokio::test]
async fn pending_device_code_expires_without_a_third_request() {
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"0"}"#,
        ),
        FakeResponse::json(404, "{}"),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url(),
        std::time::Duration::ZERO,
    );
    let cancellation = CancellationToken::new();
    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");

    let error = client
        .complete_device_code_login(&device_code, &cancellation)
        .await
        .expect_err("pending authorization should expire at its deadline");

    assert!(matches!(error, OpenAiOAuthDeviceLoginError::Expired));
    assert_eq!(issuer.finish().await.len(), 2);
}

#[tokio::test]
async fn denied_device_code_poll_stops_without_token_exchange() {
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"0"}"#,
        ),
        FakeResponse::json(400, r#"{"error":"access_denied"}"#),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url(),
        std::time::Duration::from_secs(60),
    );
    let cancellation = CancellationToken::new();
    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");

    let error = client
        .complete_device_code_login(&device_code, &cancellation)
        .await
        .expect_err("a rejected device authorization poll must stop login");

    assert!(matches!(
        error,
        OpenAiOAuthDeviceLoginError::PollRejected { status: 400 }
    ));
    assert_eq!(issuer.finish().await.len(), 2);
}

#[tokio::test]
async fn provider_entitlement_denial_rejects_device_login() {
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"0"}"#,
        ),
        FakeResponse::json(
            200,
            r#"{"authorization_code":"authorization-code","code_challenge":"challenge","code_verifier":"verifier"}"#,
        ),
        FakeResponse::json(
            400,
            r#"{"error":"access_denied","error_description":"missing_codex_entitlement"}"#,
        ),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url(),
        std::time::Duration::from_secs(60),
    );
    let cancellation = CancellationToken::new();
    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");

    let error = client
        .complete_device_code_login(&device_code, &cancellation)
        .await
        .expect_err("provider entitlement denial should reject login");

    assert!(matches!(
        error,
        OpenAiOAuthDeviceLoginError::TokenExchangeRejected { status: 400 }
    ));
    assert!(!error.to_string().contains("missing_codex_entitlement"));
    assert_eq!(issuer.finish().await.len(), 3);
}

#[tokio::test]
async fn cancelled_device_code_poll_stops_before_repolling() {
    let (pending_sent, pending_received) = oneshot::channel();
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"60"}"#,
        ),
        FakeResponse::json_and_notify(403, "{}", pending_sent),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url(),
        std::time::Duration::from_secs(900),
    );
    let cancellation = CancellationToken::new();
    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");
    let complete = client.complete_device_code_login(&device_code, &cancellation);
    tokio::pin!(complete);

    let error = tokio::select! {
        result = &mut complete => panic!("poll completed before cancellation: {result:?}"),
        result = pending_received => {
            result.expect("fake issuer should send pending response");
            cancellation.cancel();
            complete.await.expect_err("cancelled poll should not complete login")
        }
    };

    assert!(matches!(error, OpenAiOAuthDeviceLoginError::Cancelled));
    assert_eq!(issuer.finish().await.len(), 2);
}

#[tokio::test]
async fn id_token_claim_extraction_accepts_missing_chatgpt_account_id() {
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"0"}"#,
        ),
        FakeResponse::json(
            200,
            r#"{"authorization_code":"authorization-code","code_challenge":"challenge","code_verifier":"verifier"}"#,
        ),
        FakeResponse::json(
            200,
            format!(
                r#"{{"id_token":"{}","access_token":"access-token-canary","refresh_token":"refresh-token-canary"}}"#,
                real_shaped_id_token(None)
            ),
        ),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url(),
        std::time::Duration::from_secs(60),
    );
    let cancellation = CancellationToken::new();
    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");
    let credentials = client
        .complete_device_code_login(&device_code, &cancellation)
        .await
        .expect("missing optional account claim should not reject login");

    assert_eq!(credentials.chatgpt_account_id(), None);
    assert_eq!(issuer.finish().await.len(), 3);
}

fn real_shaped_id_token(chatgpt_account_id: Option<&str>) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","kid":"key-1","typ":"JWT"}"#);
    let account_claim = chatgpt_account_id.map_or_else(String::new, |account_id| {
        format!(r#", "chatgpt_account_id":"{account_id}""#)
    });
    let payload = format!(
        r#"{{"iss":"https://auth.openai.com","aud":"app_EMoamEEZ73f0CkXaXp7hrann","email":"router@example.test","https://api.openai.com/auth":{{"chatgpt_plan_type":"pro","chatgpt_user_id":"chatgpt-user-id"{account_claim}}}}}"#
    );
    format!(
        "{header}.{}.signature",
        URL_SAFE_NO_PAD.encode(payload.as_bytes())
    )
}

struct FakeDeviceIssuer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    server_task: JoinHandle<()>,
}

impl FakeDeviceIssuer {
    async fn spawn(responses: Vec<FakeResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fake issuer should bind loopback");
        let address = listener.local_addr().expect("listener should have address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_server = Arc::clone(&requests);
        let server_task = tokio::spawn(async move {
            for response in responses {
                let (stream, _) = listener
                    .accept()
                    .await
                    .expect("fake issuer should accept request");
                let (mut stream, request) = read_http_request(stream).await;
                requests_for_server
                    .lock()
                    .expect("request recording lock")
                    .push(request);
                if response.stall_after_request {
                    if let Some(notification) = response.notification {
                        let _ = notification.send(());
                    }
                    let mut client_close = [0_u8; 1];
                    let _ = stream.read(&mut client_close).await;
                    continue;
                }
                let response_bytes = format!(
                    "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response.status,
                    response.reason,
                    response.body.len(),
                    response.body
                );
                stream
                    .write_all(response_bytes.as_bytes())
                    .await
                    .expect("fake response should write");
                stream.flush().await.expect("fake response should flush");
                if let Some(notification) = response.notification {
                    let _ = notification.send(());
                }
            }
        });

        Self {
            base_url: format!("http://{address}"),
            requests,
            server_task,
        }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    async fn finish(self) -> Vec<String> {
        self.server_task
            .await
            .expect("fake issuer task should finish");
        self.requests
            .lock()
            .expect("request recording lock")
            .clone()
    }
}

struct FakeResponse {
    status: u16,
    reason: &'static str,
    body: String,
    notification: Option<oneshot::Sender<()>>,
    stall_after_request: bool,
}

impl FakeResponse {
    fn json(status: u16, body: impl Into<String>) -> Self {
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            _ => "Unexpected",
        };
        Self {
            status,
            reason,
            body: body.into(),
            notification: None,
            stall_after_request: false,
        }
    }

    fn stall_and_notify(notification: oneshot::Sender<()>) -> Self {
        Self {
            status: 200,
            reason: "OK",
            body: String::new(),
            notification: Some(notification),
            stall_after_request: true,
        }
    }

    fn json_and_notify(
        status: u16,
        body: impl Into<String>,
        notification: oneshot::Sender<()>,
    ) -> Self {
        let mut response = Self::json(status, body);
        response.notification = Some(notification);
        response
    }
}

async fn read_http_request(stream: tokio::net::TcpStream) -> (tokio::net::TcpStream, String) {
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .expect("request line should be readable");
    request.push_str(&line);

    let mut content_length = 0_usize;
    loop {
        line.clear();
        reader
            .read_line(&mut line)
            .await
            .expect("request header should be readable");
        if line == "\r\n" || line == "\n" {
            break;
        }
        let lowercase_header = line.to_ascii_lowercase();
        if let Some(value) = lowercase_header.strip_prefix("content-length:") {
            content_length = value
                .trim()
                .parse::<usize>()
                .expect("content length should be numeric");
        }
        request.push_str(&line);
    }

    let mut body = vec![0_u8; content_length];
    reader
        .read_exact(&mut body)
        .await
        .expect("request body should be complete");
    request.push_str(&String::from_utf8(body).expect("request should be UTF-8"));
    (reader.into_inner(), request)
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let unique = TEMP_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let time_nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "codex-router-openai-device-{name}-{}-{time_nonce}-{unique}",
            std::process::id(),
        ));
        fs::create_dir(&path).expect("test directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        if self.path.exists() {
            fs::remove_dir_all(&self.path).expect("test directory should be removed");
        }
    }
}
