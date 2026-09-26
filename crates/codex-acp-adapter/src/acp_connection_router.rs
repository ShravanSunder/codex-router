//! One ACP connection shell with per-Session route selection.
use crate::{
    AcpNegotiation, AcpOutputSender, AcpRouterChannels, AcpSchemaCatalog, acp_connection_channels,
    bounded_acp_output, run_acp_transport,
};
use message_board::Identity;
use serde_json::{Value, json};
use session_event_model::session_profile_codec::ProfileAdvertisement;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io,
    pin::Pin,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    task::JoinSet,
};

pub type AcpRouteFuture = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;

enum RouteOutput {
    Frame {
        endpoint: String,
        response: Value,
    },
    Closed {
        endpoint: String,
        result: io::Result<()>,
    },
}

/// An endpoint route consumes only requests for Sessions it owns. The
/// connection shell owns transport, initialize, actor identity, and routing.
pub trait AcpSessionRoute: Send {
    fn endpoint_id(&self) -> &str;
    fn run(
        self: Box<Self>,
        router: AcpRouterChannels,
        context: AcpConnectionContext,
    ) -> AcpRouteFuture;
}

#[derive(Clone)]
pub struct AcpConnectionContext {
    pub actor: Option<Identity>,
    pub client_profile: Option<ProfileAdvertisement>,
    pub client_supports_elicitation_form: bool,
}

fn rpc_error(id: Value, code: i32, message: &'static str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn selected_endpoint(method: &str, params: &Value, sessions: &BTreeMap<String, String>) -> String {
    if matches!(method, "session/new" | "session/list") {
        return params
            .pointer("/_meta/router/endpoint")
            .and_then(Value::as_str)
            .unwrap_or("codex-local")
            .to_owned();
    }
    if let Some(endpoint) = params
        .pointer("/_meta/router/sessionRef/endpoint/endpointId")
        .and_then(Value::as_str)
    {
        return endpoint.to_owned();
    }
    params
        .get("sessionId")
        .and_then(Value::as_str)
        .and_then(|session_id| sessions.get(session_id))
        .cloned()
        .unwrap_or_else(|| "codex-local".into())
}

pub async fn serve_acp_router_connection<
    TStream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
>(
    stream: TStream,
    routes: Vec<Box<dyn AcpSessionRoute>>,
) -> io::Result<()> {
    let (wire, mut connection) = acp_connection_channels();
    let closed = connection.closed.clone();
    let transport = tokio::spawn(run_acp_transport(stream, wire));
    let mut schema = AcpSchemaCatalog::load().map_err(io::Error::other)?;
    let mut negotiation = AcpNegotiation::default();
    let mut unused_routes = Some(routes);
    let mut route_inputs = BTreeMap::<String, AcpOutputSender>::new();
    let (route_output_sender, mut route_output_receiver) = mpsc::channel::<RouteOutput>(1024);
    let mut route_tasks = JoinSet::<()>::new();
    let mut session_routes = BTreeMap::<String, String>::new();
    let mut pending_routes = BTreeMap::<String, String>::new();
    let mut client_request_routes = BTreeMap::<String, String>::new();
    let mut used_ids = BTreeSet::new();

    let result = async {
        loop {
            tokio::select! {
                _ = connection.closed.cancelled() => break Ok(()),
                completed = route_tasks.join_next(), if !route_tasks.is_empty() => {
                    let Some(Ok(())) = completed else {
                        break Err(io::Error::other("ACP Session route task failed"));
                    };
                },
                routed = route_output_receiver.recv(), if !route_inputs.is_empty() => {
                    let Some(routed) = routed else {
                        break Err(io::Error::other("ACP route output unavailable"));
                    };
                    match routed {
                        RouteOutput::Frame { endpoint, response } => {
                            if response.get("method").and_then(Value::as_str).is_some()
                                && let Some(id) = response.get("id")
                                && client_request_routes.insert(id.to_string(), endpoint.clone()).is_some() {
                                break Err(io::Error::other("ACP server request ID reused"));
                            }
                            if let Some(id) = response.get("id") {
                                let key = id.to_string();
                                if pending_routes.get(&key).is_some_and(|pending| pending == &endpoint) {
                                    if let Some(session_id) = response.pointer("/result/sessionId")
                                        .and_then(Value::as_str)
                                    {
                                        session_routes.insert(session_id.to_owned(), endpoint.clone());
                                    }
                                    pending_routes.remove(&key);
                                }
                            }
                            connection.output.send(response).await?;
                        }
                        RouteOutput::Closed { endpoint, result } => {
                            route_inputs.remove(&endpoint);
                            client_request_routes.retain(|_, owner| owner != &endpoint);
                            session_routes.retain(|_, owner| owner != &endpoint);
                            let pending = pending_routes.iter()
                                .filter(|(_, owner)| *owner == &endpoint)
                                .map(|(id, _)| id.clone())
                                .collect::<Vec<_>>();
                            for key in pending {
                                pending_routes.remove(&key);
                                if let Ok(id) = serde_json::from_str(&key) {
                                    connection.output.send(rpc_error(
                                        id, -32000, "ACP endpoint unavailable"
                                    )).await?;
                                }
                            }
                            if route_inputs.is_empty() {
                                break result;
                            }
                        }
                    }
                },
                incoming = connection.input.recv() => {
                    let Some(frame) = incoming else { break Ok(()); };
                    if frame.get("jsonrpc") != Some(&json!("2.0")) {
                        connection.output.send(rpc_error(Value::Null, -32600, "Invalid ACP envelope")).await?;
                        continue;
                    }
                    let Some(method) = frame.get("method").and_then(Value::as_str) else {
                        if let Some(id) = frame.get("id")
                            && let Some(endpoint) = client_request_routes.remove(&id.to_string())
                            && let Some(route) = route_inputs.get(&endpoint) {
                            route.send((*frame).clone()).await?;
                        }
                        continue;
                    };
                    let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));
                    let Some(id) = frame.get("id").cloned() else {
                        if negotiation.is_initialized() && method == "session/cancel" {
                            let endpoint = selected_endpoint(method, &params, &session_routes);
                            if let Some(route) = route_inputs.get(&endpoint) {
                                route.send((*frame).clone()).await?;
                            }
                        }
                        continue;
                    };
                    if !(id.is_null() || id.is_string() || id.as_i64().is_some()) {
                        connection.output.send(rpc_error(Value::Null, -32600, "Invalid ACP request ID")).await?;
                        continue;
                    }
                    if used_ids.len() >= 65536 {
                        break Err(io::Error::other("ACP request lifetime limit"));
                    }
                    if !used_ids.insert(id.to_string()) {
                        connection.output.send(rpc_error(id, -32600, "ACP request ID reused")).await?;
                        continue;
                    }
                    if method == "initialize" {
                        let actor = params.pointer("/_meta/router/actor")
                            .map(|value| serde_json::from_value::<Identity>(value.clone()))
                            .transpose();
                        let Ok(actor) = actor else {
                            connection.output.send(rpc_error(id, -32602, "ACP initialization rejected")).await?;
                            continue;
                        };
                        let response = negotiation.initialize(&mut schema, &frame)
                            .unwrap_or_else(|_| rpc_error(id, -32602, "ACP initialization rejected"));
                        let initialized = response.get("result").is_some();
                        connection.output.send(response).await?;
                        if initialized && let Some(routes) = unused_routes.take() {
                            let context = AcpConnectionContext {
                                actor,
                                client_profile: params.pointer("/_meta/sessionProfile")
                                    .and_then(|profile| serde_json::from_value(profile.clone()).ok()),
                                client_supports_elicitation_form: params
                                    .pointer("/clientCapabilities/elicitation/form")
                                    .is_some_and(Value::is_object),
                            };
                            for route in routes {
                                let endpoint = route.endpoint_id().to_owned();
                                let (input_sender, input_receiver) =
                                    bounded_acp_output(connection.closed.child_token());
                                let (output_sender, mut output_receiver) =
                                    bounded_acp_output(connection.closed.child_token());
                                let route_channels = AcpRouterChannels {
                                    input: input_receiver,
                                    output: output_sender,
                                    closed: connection.closed.child_token(),
                                };
                                let output = route_output_sender.clone();
                                let route_endpoint = endpoint.clone();
                                let route_context = context.clone();
                                route_tasks.spawn(async move {
                                    let forward = async {
                                        while let Some(frame) = output_receiver.recv().await {
                                            if output.send(RouteOutput::Frame {
                                                endpoint: route_endpoint.clone(),
                                                response: (*frame).clone(),
                                            }).await.is_err() {
                                                return Err(io::Error::other("ACP route output closed"));
                                            }
                                        }
                                        Ok(())
                                    };
                                    let (route_result, forward_result) =
                                        tokio::join!(route.run(route_channels, route_context), forward);
                                    let result = route_result.and(forward_result);
                                    let _sent = output.send(RouteOutput::Closed {
                                        endpoint: route_endpoint, result,
                                    }).await;
                                });
                                route_inputs.insert(endpoint, input_sender);
                            }
                        }
                        continue;
                    }
                    if !negotiation.is_initialized() {
                        connection.output.send(rpc_error(id, -32600, "ACP initialization required")).await?;
                        continue;
                    }
                    let endpoint = selected_endpoint(method, &params, &session_routes);
                    let Some(route) = route_inputs.get(&endpoint) else {
                        connection.output.send(rpc_error(id, -32602, "ACP endpoint unavailable")).await?;
                        continue;
                    };
                    pending_routes.insert(id.to_string(), endpoint);
                    route.send((*frame).clone()).await?;
                }
            }
        }
    }.await;
    closed.cancel();
    route_tasks.abort_all();
    while route_tasks.join_next().await.is_some() {}
    let transport_result = transport
        .await
        .map_err(|_| io::Error::other("ACP transport task failed"))?;
    result.and(transport_result)
}
