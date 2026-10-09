//! The collaboration API: one stateless MCP server at `/mcp`, mounted on an axum router and
//! served on whichever listener the Host hands it. Every call stands alone: there is no session
//! manager and no connection state, and each listener sheds load past its own request limit.
use crate::mcp_server::{CollaborationMcpServer, ToolSurface};
use crate::native_schema_definitions::NativeSchemaDefinitions;
use crate::tool_call_registry::ToolCallRegistry;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use collaboration_protocol::RouterExecutableRelation;
use collaboration_service::CollaborationApplication;
use rmcp::{
    model::{
        CallToolResult, ClientJsonRpcMessage, ClientRequest, ErrorCode, ErrorData,
        ServerJsonRpcMessage, ServerResult,
    },
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    },
};
use std::{io, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio_util::sync::CancellationToken;

/// The path every listener serves the collaboration API on.
pub const COLLABORATION_API_PATH: &str = "/mcp";
/// How many requests one listener runs at once before shedding the rest.
pub const DEFAULT_CONCURRENT_REQUESTS: usize = 64;
/// How long shutdown lets running tool calls finish on their own after it cancels them.
const CALL_GRACE: Duration = Duration::from_secs(10);
/// How long shutdown waits for aborted calls' connections and tasks to end.
const ABORTED_CALL_DRAIN: Duration = Duration::from_secs(2);
/// The largest request body a listener reads. The biggest tool arguments, message text and
/// a schedule package, are bounded at `MAX_CONTROL_FRAME_BYTES` (1 MiB) before their JSON
/// envelope and escaping, so 4 MiB leaves room without letting one request hold an
/// unbounded buffer. A shed request is read up to the same bound to name it in the overload
/// answer.
const MAX_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;

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
    tool_calls: ToolCallRegistry,
    call_grace: Duration,
}

impl CollaborationApiRouter {
    /// The tool calls this copy runs, to count them.
    #[cfg(test)]
    pub(crate) fn tool_calls(&self) -> ToolCallRegistry {
        self.tool_calls.clone()
    }

    /// The same copy with a shorter shutdown grace period.
    #[cfg(test)]
    pub(crate) fn with_call_grace(mut self, grace: Duration) -> Self {
        self.call_grace = grace;
        self
    }
}

/// Builds one listener's copy of the stateless collaboration API.
#[must_use]
pub fn collaboration_api_router(
    config: &CollaborationApiConfig,
    listener: CollaborationApiListener,
) -> CollaborationApiRouter {
    let mut service_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(MAX_REQUEST_BODY_BYTES)
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
    let tool_calls = ToolCallRegistry::new(config.shutdown.clone());
    let server_tool_calls = tool_calls.clone();
    let surface = Arc::new(ToolSurface::new(config.native_definitions.as_ref()));
    let service: StreamableHttpService<CollaborationMcpServer, NeverSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok(CollaborationMcpServer::new(
                    &server_config,
                    Arc::clone(&surface),
                    server_tool_calls.clone(),
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
        tool_calls,
        call_grace: CALL_GRACE,
    }
}

/// Serves one listener's copy until `shutdown` is cancelled, then stops its calls: they see
/// the cancellation and get a grace period to finish, the rest are aborted, and the listener
/// returns only once no call is running, so no call outlives it.
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
        tool_calls,
        call_grace,
        ..
    } = api;
    let serving = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .into_future();
    let mut serving = std::pin::pin!(serving);
    let served = tokio::select! {
        served = &mut serving => served,
        () = shutdown.cancelled() => {
            match tokio::time::timeout(call_grace, &mut serving).await {
                Ok(served) => served,
                Err(_) => {
                    tool_calls.abort_all();
                    tokio::time::timeout(ABORTED_CALL_DRAIN, &mut serving)
                        .await
                        .unwrap_or_else(|_| Err(not_stopped("connections")))
                }
            }
        }
    };
    // A call whose caller has gone can still be running after its connection closed.
    if tokio::time::timeout(call_grace, tool_calls.ended())
        .await
        .is_err()
    {
        tool_calls.abort_all();
        tokio::time::timeout(ABORTED_CALL_DRAIN, tool_calls.ended())
            .await
            .map_err(|_| not_stopped("tool calls"))?;
    }
    served
}

fn not_stopped(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("collaboration API {what} did not stop after their calls were aborted"),
    )
}

/// A listener's request slot, shared by the request and the tool call it runs.
#[derive(Clone)]
pub(crate) struct CallAdmission {
    _permit: Arc<OwnedSemaphorePermit>,
}

/// Admits a request if this listener has capacity; otherwise answers it as `overloaded`
/// without running it.
///
/// A tower `ConcurrencyLimit` with `LoadShed` would shed the same requests, but its error
/// handler sees no request body, so it could not answer a shed tool call with a tool result
/// carrying that call's id. The slot is held until both the answer's headers are out and the
/// tool call has ended: a streamed (SSE) call keeps its share while it runs, and a reader
/// that stops reading the rest of the stream does not keep the slot after the call ends.
async fn shed_past_capacity(
    State(admission): State<Arc<Semaphore>>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Ok(permit) = Arc::clone(&admission).try_acquire_owned() {
        let admitted = CallAdmission {
            _permit: Arc::new(permit),
        };
        request.extensions_mut().insert(admitted.clone());
        let response = next.run(request).await;
        drop(admitted);
        return response;
    }
    let (parts, body) = request.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_REQUEST_BODY_BYTES).await else {
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
