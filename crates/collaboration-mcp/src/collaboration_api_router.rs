//! The collaboration API: one stateless MCP server at `/mcp`, mounted on an axum router and
//! served on whichever listener the Host hands it. Every call stands alone: there is no session
//! manager and no connection state, and each listener sheds load past its own request limit.
use crate::mcp_server::{CollaborationMcpServer, ToolSurface};
use crate::native_schema_definitions::NativeSchemaDefinitions;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use collaboration_protocol::RouterExecutableRelation;
use collaboration_service::CollaborationApplication;
use futures_util::StreamExt;
use rmcp::{
    model::{
        CallToolResult, ClientJsonRpcMessage, ClientRequest, ErrorCode, ErrorData,
        ServerJsonRpcMessage, ServerResult,
    },
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    },
};
use std::{
    io,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

/// The path every listener serves the collaboration API on.
pub const COLLABORATION_API_PATH: &str = "/mcp";
/// How many requests one listener runs at once before shedding the rest.
pub const DEFAULT_CONCURRENT_REQUESTS: usize = 64;
/// How long shutdown waits for cancelled tool calls to release the Router's stores.
const CALL_SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
/// The most bytes of a shed request read to name it in the overload answer.
const SHED_REQUEST_READ_LIMIT: usize = 4 * 1024 * 1024;

/// What every listener's copy of the collaboration API serves from.
#[derive(Clone)]
pub struct CollaborationApiConfig {
    pub application: CollaborationApplication,
    /// The service directory, where the carrier sockets the conversation and observation
    /// tools open live.
    pub service_directory: PathBuf,
    /// Codex native schema definitions that tool schemas' native references bind to.
    pub native_definitions: Option<NativeSchemaDefinitions>,
    pub router_executable_relation: watch::Receiver<RouterExecutableRelation>,
    /// Requests one listener runs at once; requests beyond it are shed as `overloaded`.
    pub concurrent_requests: usize,
    /// Cancelling it cancels every in-flight tool call and stops every listener.
    pub shutdown: CancellationToken,
}

/// The listener a router copy is served on, which decides the origins it accepts.
#[derive(Clone, Copy, Debug)]
pub enum CollaborationApiListener {
    /// Localhost TCP, for models; browsers on this machine may only use this exact origin.
    LoopbackTcp(SocketAddr),
    /// The owner-only Unix socket, for the CLIs; no browser origin is accepted.
    UnixSocket,
}

/// One listener's copy of the collaboration API, ready to serve.
pub struct CollaborationApiRouter {
    router: Router,
    active_calls: Arc<AtomicUsize>,
}

impl CollaborationApiRouter {
    /// Tool handlers this copy has in flight.
    #[cfg(test)]
    pub(crate) fn active_calls(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.active_calls)
    }
}

/// Builds one listener's copy of the stateless collaboration API.
#[must_use]
pub fn collaboration_api_router(
    config: &CollaborationApiConfig,
    listener: CollaborationApiListener,
) -> CollaborationApiRouter {
    let active_calls = Arc::new(AtomicUsize::new(0));
    let mut service_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_cancellation_token(config.shutdown.child_token());
    service_config = match listener {
        CollaborationApiListener::LoopbackTcp(address) => service_config
            .with_allowed_hosts([address.ip().to_string(), "localhost".to_owned()])
            .with_allowed_origins([
                format!("http://{address}"),
                format!("http://localhost:{}", address.port()),
            ])
            .enforce_origin_validation(),
        CollaborationApiListener::UnixSocket => service_config.enforce_origin_validation(),
    };
    let server_config = config.clone();
    let server_calls = Arc::clone(&active_calls);
    let surface = Arc::new(ToolSurface::new(config.native_definitions.as_ref()));
    let service: StreamableHttpService<CollaborationMcpServer, NeverSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok(CollaborationMcpServer::new(
                    &server_config,
                    Arc::clone(&surface),
                    Arc::clone(&server_calls),
                ))
            },
            Arc::new(NeverSessionManager::default()),
            service_config,
        );
    let admission = Arc::new(Semaphore::new(config.concurrent_requests));
    let router = Router::new()
        .route_service(COLLABORATION_API_PATH, service)
        .layer(middleware::from_fn_with_state(
            admission,
            shed_past_capacity,
        ));
    CollaborationApiRouter {
        router,
        active_calls,
    }
}

/// Serves one listener's copy until `shutdown` is cancelled, then waits for its cancelled
/// calls to finish so the Router's stores are released.
pub async fn serve_collaboration_api<TListener>(
    listener: TListener,
    api: CollaborationApiRouter,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    TListener: axum::serve::Listener,
    TListener::Addr: std::fmt::Debug,
{
    let CollaborationApiRouter {
        router,
        active_calls,
    } = api;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await?;
    let settled = tokio::time::timeout(CALL_SETTLE_TIMEOUT, async {
        while active_calls.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    settled.map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "collaboration API calls did not settle after shutdown",
        )
    })
}

/// Admits a request if this listener has capacity; otherwise answers it as `overloaded`
/// without running it.
///
/// A tower `ConcurrencyLimit` with `LoadShed` would shed the same requests, but its error
/// handler sees no request body, so it could not answer a shed tool call with a tool result
/// carrying that call's id. A streamed (SSE) answer keeps its permit until the stream ends; a
/// JSON answer is complete when the handler returns.
async fn shed_past_capacity(
    State(admission): State<Arc<Semaphore>>,
    request: Request,
    next: Next,
) -> Response {
    if let Ok(permit) = Arc::clone(&admission).try_acquire_owned() {
        let response = next.run(request).await;
        let streamed = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if !streamed {
            return response;
        }
        let (parts, body) = response.into_parts();
        let body = Body::from_stream(body.into_data_stream().map(move |chunk| {
            let _admitted = &permit;
            chunk
        }));
        return Response::from_parts(parts, body);
    }
    let (parts, body) = request.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, SHED_REQUEST_READ_LIMIT).await else {
        return overloaded_http_response();
    };
    match serde_json::from_slice::<ClientJsonRpcMessage>(&bytes) {
        Ok(ClientJsonRpcMessage::Request(request)) => {
            let message = match request.request {
                ClientRequest::CallToolRequest(_) => ServerJsonRpcMessage::response(
                    ServerResult::CallToolResult(overloaded_tool_result()),
                    request.id,
                ),
                _ => ServerJsonRpcMessage::error(overloaded_error(), Some(request.id)),
            };
            json_message_response(&message)
        }
        // Notifications and responses carry no work to shed.
        Ok(_) => next_with_body(parts, bytes, next).await,
        Err(_) => overloaded_http_response(),
    }
}

async fn next_with_body(parts: axum::http::request::Parts, bytes: Bytes, next: Next) -> Response {
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

const OVERLOADED_MESSAGE: &str = "Request capacity exceeded; this request was not run. Retry the same operation identity when capacity is available.";

/// The typed tool error a shed tool call receives.
fn overloaded_tool_result() -> CallToolResult {
    crate::mcp_server::structured_tool_error(serde_json::json!({
        "kind": "overloaded",
        "stage": "admission",
        "effect": "none",
        "message": OVERLOADED_MESSAGE,
    }))
}

fn overloaded_error() -> ErrorData {
    ErrorData::new(
        ErrorCode(-32050),
        OVERLOADED_MESSAGE,
        Some(
            serde_json::json!({"kind":"overloaded","stage":"admission","message":OVERLOADED_MESSAGE}),
        ),
    )
}

fn json_message_response(message: &ServerJsonRpcMessage) -> Response {
    match serde_json::to_vec(message) {
        Ok(body) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response(),
        Err(_) => overloaded_http_response(),
    }
}

fn overloaded_http_response() -> Response {
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        OVERLOADED_MESSAGE,
    )
        .into_response()
}
