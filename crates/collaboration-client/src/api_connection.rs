//! One stateless tool call per HTTP request over the owner-only API socket.
//!
//! The collaboration API is a stateless MCP server: a call needs no handshake and no session,
//! so each call is a single `tools/call` request on its own Unix-socket connection, made with
//! rmcp's Unix-socket HTTP client. Dropping a call closes its connection, which cancels it on
//! the server.
use crate::ClientError;
use futures_util::StreamExt;
use rmcp::{
    model::{
        CallToolRequest, CallToolRequestParams, ClientJsonRpcMessage, ClientRequest,
        NumberOrString, ServerJsonRpcMessage, ServerNotification, ServerResult,
    },
    transport::{
        UnixSocketHttpClient,
        streamable_http_client::{StreamableHttpClient, StreamableHttpPostResponse},
    },
};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};

/// The URI every call names; the socket, not the host, selects the server.
const API_URI: &str = "http://localhost/mcp";
/// The MCP revision whose tool results carry `structuredContent`.
const API_PROTOCOL_VERSION: &str = "2025-11-25";
/// The default bound on one call; waits pass their own.
pub(crate) const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A tool's answer: its structured result, or its structured tool error.
#[derive(Debug)]
pub(crate) enum ToolAnswer {
    Success(Value),
    Failure(Value),
}

#[derive(Clone)]
pub(crate) struct ApiConnection {
    client: UnixSocketHttpClient,
    uri: Arc<str>,
}

impl ApiConnection {
    pub(crate) fn new(socket: &str) -> Self {
        Self {
            client: UnixSocketHttpClient::new(socket, API_URI),
            uri: Arc::from(API_URI),
        }
    }

    /// Calls `tool`, answering its structured result; a tool error becomes the rejection the
    /// tool published.
    pub(crate) async fn call(&self, tool: &str, arguments: Value) -> Result<Value, ClientError> {
        self.call_with_timeout(tool, arguments, DEFAULT_CALL_TIMEOUT)
            .await
    }

    pub(crate) async fn call_with_timeout(
        &self,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        match self.answer_with_timeout(tool, arguments, timeout).await? {
            ToolAnswer::Success(value) => Ok(value),
            ToolAnswer::Failure(failure) => Err(rejection_from_tool_failure(failure)),
        }
    }

    /// Calls `tool` and answers its result or tool error as the tool returned it.
    pub(crate) async fn answer(
        &self,
        tool: &str,
        arguments: Value,
    ) -> Result<ToolAnswer, ClientError> {
        self.answer_with_timeout(tool, arguments, DEFAULT_CALL_TIMEOUT)
            .await
    }

    pub(crate) async fn answer_with_timeout(
        &self,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<ToolAnswer, ClientError> {
        tokio::time::timeout(timeout, self.exchange(tool, arguments, None))
            .await
            .map_err(|_| ClientError::Timeout)?
    }

    /// Calls a streaming tool: each notification the server sends before the answer goes to
    /// `notified` as it arrives, then the answer is read as `call_with_timeout` reads it.
    pub(crate) async fn call_observing(
        &self,
        tool: &str,
        arguments: Value,
        timeout: Duration,
        notified: &(dyn Fn(ServerNotification) + Send + Sync),
    ) -> Result<Value, ClientError> {
        let answer = tokio::time::timeout(timeout, self.exchange(tool, arguments, Some(notified)))
            .await
            .map_err(|_| ClientError::Timeout)??;
        match answer {
            ToolAnswer::Success(value) => Ok(value),
            ToolAnswer::Failure(failure) => Err(rejection_from_tool_failure(failure)),
        }
    }

    async fn exchange(
        &self,
        tool: &str,
        arguments: Value,
        notified: Option<&(dyn Fn(ServerNotification) + Send + Sync)>,
    ) -> Result<ToolAnswer, ClientError> {
        let Value::Object(arguments) = arguments else {
            return Err(ClientError::InvalidRequest(
                "tool arguments must be an object",
            ));
        };
        let id = NumberOrString::Number(1);
        let request = ClientJsonRpcMessage::request(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments),
            )),
            id.clone(),
        );
        let mut headers = HashMap::new();
        headers.insert(
            http::HeaderName::from_static("mcp-protocol-version"),
            http::HeaderValue::from_static(API_PROTOCOL_VERSION),
        );
        let response = self
            .client
            .post_message(Arc::clone(&self.uri), request, None, None, headers)
            .await
            .map_err(|error| transport_failure(&error))?;
        let message = match response {
            StreamableHttpPostResponse::Json(message, _) => message,
            StreamableHttpPostResponse::Sse(mut events, _) => loop {
                let Some(event) = events.next().await else {
                    return Err(ClientError::Protocol("API stream ended without an answer"));
                };
                let event = event.map_err(|_| ClientError::Protocol("invalid API stream"))?;
                let Some(data) = event.data.filter(|data| !data.is_empty()) else {
                    continue;
                };
                let message: ServerJsonRpcMessage = serde_json::from_str(&data)
                    .map_err(|_| ClientError::Protocol("invalid API stream message"))?;
                match message {
                    ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_) => {
                        break message;
                    }
                    ServerJsonRpcMessage::Notification(notification) => {
                        if let Some(notified) = notified {
                            notified(notification.notification);
                        }
                    }
                    ServerJsonRpcMessage::Request(_) => {}
                }
            },
            _ => {
                return Err(ClientError::Protocol(
                    "API accepted a call without answering",
                ));
            }
        };
        match message {
            ServerJsonRpcMessage::Response(response) if response.id == id => {
                let ServerResult::CallToolResult(result) = response.result else {
                    return Err(ClientError::Protocol("API answered with a non-tool result"));
                };
                let structured = result
                    .structured_content
                    .ok_or(ClientError::Protocol("API tool result is not structured"))?;
                Ok(if result.is_error == Some(true) {
                    ToolAnswer::Failure(structured)
                } else {
                    ToolAnswer::Success(structured)
                })
            }
            ServerJsonRpcMessage::Error(error) => Err(ClientError::Rejected {
                code: i64::from(error.error.code.0),
                data: error.error.data,
            }),
            _ => Err(ClientError::Protocol("API answer does not match the call")),
        }
    }
}

/// The rejection a tool error carries.
///
/// A rejection the tool reports as an operation failure keeps the code and payload the Router
/// published; any other tool error is the family's typed failure itself.
pub(crate) fn rejection_from_tool_failure(mut failure: Value) -> ClientError {
    if let Some(overload) = crate::admission_overload::admission_overload(&failure) {
        return overload;
    }
    if let Some(fields) = failure.as_object_mut() {
        fields.remove("mcpResult");
        if fields.get("kind").and_then(Value::as_str) == Some("rejected")
            && let Some(code) = fields.get("code").and_then(Value::as_i64)
        {
            let data = fields.remove("data").filter(|data| !data.is_null());
            return ClientError::Rejected { code, data };
        }
    }
    ClientError::Rejected {
        code: OPERATION_FAILED,
        data: Some(failure),
    }
}

/// The code under which the Router publishes typed operation failures.
pub(crate) const OPERATION_FAILED: i64 = -32050;

fn transport_failure(
    error: &rmcp::transport::streamable_http_client::StreamableHttpError<
        rmcp::transport::common::unix_socket::UnixSocketError,
    >,
) -> ClientError {
    use rmcp::transport::{
        common::unix_socket::UnixSocketError, streamable_http_client::StreamableHttpError,
    };
    match error {
        StreamableHttpError::Io(source)
        | StreamableHttpError::Client(UnixSocketError::Io(source)) => {
            ClientError::Transport(std::io::Error::new(source.kind(), source.to_string()))
        }
        StreamableHttpError::Client(source) => {
            ClientError::Transport(std::io::Error::other(source.to_string()))
        }
        StreamableHttpError::UnexpectedServerResponse(_) => {
            ClientError::Protocol("API refused the request")
        }
        _ => ClientError::Protocol("API transport failed"),
    }
}
