//! Opt-in provider proof through Router's real request preparation and Hyper transport.
//! The selected identity and memory credential replace only account/OAuth dependencies.
//! No production account, provider identity, profile or credential store is changed.

use bytes::Bytes;
use codex_router_auth::resolver::{CredentialResolverError, ResolvedProviderCredential};
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::ids::{AccountId, TokenGeneration};
use codex_router_core::local_auth::{LocalRouterAuth, LocalRouterTokenRecord};
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_proxy::account_selection::{
    AsyncAccountDecisionSelector, SelectedAccountDecision,
};
use codex_router_proxy::headers::Header;
use codex_router_proxy::http_sse::{
    AsyncProviderCredentialResolver, AsyncStreamingUpstreamHttpTransport,
    AuthenticatedHttpProxyService, HttpAffinitySecretProvider, HttpProxyError, HttpProxyRequest,
};
use codex_router_proxy::local_auth::ProxyLocalAuthGate;
use codex_router_proxy::routes::Method;
use codex_router_proxy::upstream::{HyperHttpUpstreamTransport, UpstreamEndpoint};
use futures_util::future::BoxFuture;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use std::cell::Cell;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;

const TEST_ACCOUNT: &str = "selector_openrouter_probe";
const MODEL_NAME: &str = "anthropic/claude-haiku-5.5";
const MARKER_TEXT: &str = "ROUTER_SELECTOR_PROVIDER_OK";
const LOCAL_TOKEN: &str = "selector-local-auth-canary";
const OFFLINE_KEY: &str = "selector-memory-key-canary";
const RESPONSE_LIMIT: usize = 1024 * 1024;

struct MemoryTestCredential {
    key: SecretString,
    calls: AtomicUsize,
}

impl AsyncProviderCredentialResolver for MemoryTestCredential {
    fn resolve_provider_credentials<'a>(
        &'a self,
        account_id: &'a AccountId,
        expected_provider: Provider,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        Box::pin(async move {
            if account_id.as_str() != TEST_ACCOUNT || expected_provider != Provider::Openai {
                return Err(CredentialResolverError::AccountProviderMismatch);
            }
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                self.key.clone(),
                1,
            ))
        })
    }
}

struct IsolatedTestSelection;

impl AsyncAccountDecisionSelector for IsolatedTestSelection {
    fn select_upstream_account<'a>(
        &'a self,
        _request: &'a HttpProxyRequest,
        _generation: TokenGeneration,
        _affinity: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            AccountId::new(TEST_ACCOUNT)
                .map(|account| SelectedAccountDecision::new(account, "isolated-provider-proof"))
                .map_err(|_| HttpProxyError::ProviderCredential {
                    reason: CredentialResolverError::AccountUnavailable,
                })
        })
    }
}

struct MemoryTestAffinity;

impl HttpAffinitySecretProvider for MemoryTestAffinity {
    fn load_or_create_affinity_secret(&self) -> Result<RouterAffinityHashSecret, HttpProxyError> {
        RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .map_err(|_| HttpProxyError::ProviderCredential {
            reason: CredentialResolverError::SecretUnavailable,
        })
    }
}

fn require(condition: bool, message: &'static str) -> Result<(), &'static str> {
    if condition { Ok(()) } else { Err(message) }
}

fn request_body() -> Vec<u8> {
    format!(
        r#"{{"model":"{MODEL_NAME}","input":"Reply exactly {MARKER_TEXT}. No tools.","max_output_tokens":128,"stream":false}}"#
    )
    .into_bytes()
}

struct ObservedResponse {
    status: u16,
    body: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
enum ForwardStage {
    Preparing,
    AwaitingHeaders,
    ReadingBody,
    Complete,
}

async fn forward_once(
    endpoint: UpstreamEndpoint,
    credential: &MemoryTestCredential,
    stage: &Cell<ForwardStage>,
) -> Result<ObservedResponse, &'static str> {
    let auth = ProxyLocalAuthGate::required(LocalRouterAuth::new(
        LocalRouterTokenRecord::new(SecretString::new(LOCAL_TOKEN), TokenGeneration::new(1)),
        Vec::new(),
    ));
    let selector = IsolatedTestSelection;
    let affinity = MemoryTestAffinity;
    let transport = HyperHttpUpstreamTransport::new(endpoint);
    let service = AuthenticatedHttpProxyService::new(&auth, &selector, credential, &transport)
        .with_affinity_secret_provider(&affinity);
    let body = request_body();
    let request = HttpProxyRequest::new(Method::Post, "/v1/responses")
        .with_header(Header::new("x-codex-router-token", LOCAL_TOKEN))
        .with_header(Header::new(
            "authorization",
            format!("Bearer {LOCAL_TOKEN}"),
        ))
        .with_header(Header::new("content-type", "application/json"))
        .with_body(body.clone());
    let prepared = service
        .prepare_async_streaming_request_async(
            request,
            Full::new(Bytes::from(body))
                .map_err(|never| match never {})
                .boxed(),
        )
        .await
        .map_err(|_| "Router request preparation failed")?;
    let (request, _completion) = prepared.into_parts();
    stage.set(ForwardStage::AwaitingHeaders);
    let response = transport
        .send_streaming(request)
        .await
        .map_err(|_| "Router upstream transport failed")?;
    let (status, _headers, mut stream) = response.into_parts();
    stage.set(ForwardStage::ReadingBody);
    let mut body = Vec::new();
    while let Some(frame) = stream.frame().await {
        let frame = frame.map_err(|_| "Router response stream failed")?;
        if let Ok(bytes) = frame.into_data() {
            require(
                body.len() + bytes.len() <= RESPONSE_LIMIT,
                "response exceeds bound",
            )?;
            body.extend_from_slice(&bytes);
        }
    }
    stage.set(ForwardStage::Complete);
    Ok(ObservedResponse { status, body })
}

#[derive(Deserialize)]
struct ResponsesEnvelope {
    model: String,
    status: String,
    output: Vec<ResponseItem>,
}

#[derive(Deserialize)]
struct ResponseItem {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Vec<ResponseContent>,
}

#[derive(Deserialize)]
struct ResponseContent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

fn verify_response(response: &ObservedResponse) -> Result<(), &'static str> {
    require(response.status == 200, "provider returned non-200 status")?;
    let envelope: ResponsesEnvelope =
        serde_json::from_slice(&response.body).map_err(|_| "invalid Responses envelope")?;
    require(
        envelope.model == MODEL_NAME,
        "provider model does not match requested binding",
    )?;
    require(
        envelope.status == "completed",
        "provider response was not completed",
    )?;
    let text = envelope
        .output
        .iter()
        .filter(|item| item.kind == "message")
        .flat_map(|item| &item.content)
        .filter(|content| content.kind == "output_text")
        .map(|content| content.text.as_str())
        .collect::<String>();
    require(text.trim() == MARKER_TEXT, "provider marker did not match")
}

#[tokio::test]
async fn router_memory_key_and_model_reach_real_http_peer() -> Result<(), &'static str> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "owned listener bind failed")?;
    let address = listener
        .local_addr()
        .map_err(|_| "listener address unavailable")?;
    let observations = Arc::new(AtomicUsize::new(0));
    let server_observations = Arc::clone(&observations);
    let mut server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.map_err(|_| "owned accept failed")?;
        let handler = service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
            let observations = Arc::clone(&server_observations);
            async move {
                let auth_ok = request.headers().get_all("authorization").iter().count() == 1
                    && request.headers().get("authorization").is_some_and(|value| {
                        value.as_bytes() == format!("Bearer {OFFLINE_KEY}").as_bytes()
                    });
                let privacy_ok = !request.headers().contains_key("x-codex-router-token")
                    && !request.headers().contains_key("chatgpt-account-id");
                let path_ok = request.uri().path() == "/api/v1/responses";
                let body_ok = request
                    .into_body()
                    .collect()
                    .await
                    .is_ok_and(|body| {
                        body.to_bytes().as_ref()
                            == br#"{"model":"anthropic/claude-haiku-5.5","input":"Reply exactly ROUTER_SELECTOR_PROVIDER_OK. No tools.","max_output_tokens":128,"stream":false}"#
                    });
                if auth_ok && privacy_ok && path_ok && body_ok {
                    observations.fetch_add(1, Ordering::Relaxed);
                }
                let response = r#"{"model":"anthropic/claude-haiku-5.5","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"ROUTER_SELECTOR_PROVIDER_OK"}]}]}"#;
                Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(response))))
            }
        });
        hyper::server::conn::http1::Builder::new()
            .keep_alive(false)
            .serve_connection(TokioIo::new(stream), handler)
            .await
            .map_err(|_| "owned peer connection failed")
    });
    let credential = MemoryTestCredential {
        key: SecretString::new(OFFLINE_KEY),
        calls: AtomicUsize::new(0),
    };
    let endpoint = UpstreamEndpoint::new(format!("http://{address}/api/v1"))
        .map_err(|_| "invalid owned endpoint")?;
    let stage = Cell::new(ForwardStage::Preparing);
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        forward_once(endpoint, &credential, &stage),
    )
    .await
    .map_err(|_| "offline proxy deadline exceeded")?;
    // Join or cancel only this test's owned peer, including on preparation failure.
    if response.is_err() {
        server.abort();
        let _ = server.await;
    } else {
        let joined = tokio::time::timeout(Duration::from_secs(10), &mut server).await;
        match joined {
            Ok(result) => result.map_err(|_| "owned server join failed")??,
            Err(_) => {
                server.abort();
                let _ = server.await;
                return Err("owned server deadline exceeded");
            }
        }
    }
    require(
        credential.calls.load(Ordering::Relaxed) == 1,
        "credential boundary was not reached once",
    )?;
    verify_response(&response?)?;
    require(
        observations.load(Ordering::Relaxed) == 1,
        "peer did not observe exact bound request",
    )?;
    require(
        credential.calls.load(Ordering::Relaxed) == 1,
        "credential resolver called more than once",
    )
}

#[tokio::test]
async fn memory_key_binding_rejects_other_identities_before_access() -> Result<(), &'static str> {
    let credential = MemoryTestCredential {
        key: SecretString::new(OFFLINE_KEY),
        calls: AtomicUsize::new(0),
    };
    for (identity, provider) in [
        ("different-account", Provider::Openai),
        (TEST_ACCOUNT, Provider::Claude),
    ] {
        let account = AccountId::new(identity).map_err(|_| "invalid test account")?;
        require(
            matches!(
                credential
                    .resolve_provider_credentials(&account, provider)
                    .await,
                Err(CredentialResolverError::AccountProviderMismatch)
            ),
            "memory key binding accepted the wrong identity or protocol",
        )?;
    }
    require(
        credential.calls.load(Ordering::Relaxed) == 0,
        "rejected request accessed the memory key",
    )
}

#[tokio::test]
#[ignore = "one owner-authorized live OpenRouter request; key supplied only in child memory"]
async fn live_router_openrouter_haiku_marker() -> Result<(), &'static str> {
    let key = std::env::var("OPENROUTER_API_KEY").map_err(|_| "live test key unavailable")?;
    require(!key.trim().is_empty(), "live test key empty")?;
    let credential = MemoryTestCredential {
        key: SecretString::new(key),
        calls: AtomicUsize::new(0),
    };
    let endpoint = UpstreamEndpoint::new("https://openrouter.ai/api/v1")
        .map_err(|_| "invalid fixed live endpoint")?;
    let stage = Cell::new(ForwardStage::Preparing);
    let outcome = tokio::time::timeout(
        Duration::from_secs(60),
        forward_once(endpoint, &credential, &stage),
    )
    .await;
    println!(
        "provider-seam-stage: phase={:?} resolverCalls={}",
        stage.get(),
        credential.calls.load(Ordering::Relaxed)
    );
    let response = outcome.map_err(|_| "live proxy deadline exceeded; no retry")??;
    // Never emit upstream body, headers, key or a raw transport error.
    println!(
        "provider-seam: model={MODEL_NAME} status={} resolverCalls={} responseBytes={}",
        response.status,
        credential.calls.load(Ordering::Relaxed),
        response.body.len()
    );
    require(
        !response
            .body
            .windows(credential.key.expose_secret().len())
            .any(|window| window == credential.key.expose_secret().as_bytes()),
        "provider response contained credential material",
    )?;
    require(
        credential.calls.load(Ordering::Relaxed) == 1,
        "live resolver called more than once",
    )?;
    verify_response(&response)
}
