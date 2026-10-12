//! Read the existing stateless v3 JSON MCP API; no session, credential or recovery owner.
use super::picker_runtime_inventory::native_inventory_pager::NativeInventoryReadClient;
use super::router_connection_registry::RouterConnectionProfile;
use collaboration_client::{
    ClientError,
    protocol::{
        EndpointInventory, NativeSessionListParams, NativeSessionListResult,
        ProviderSessionListParams, ProviderSessionListResult,
    },
};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientJsonRpcMessage, ClientRequest, NumberOrString,
    ServerJsonRpcMessage, ServerResult,
};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

const READ_TIMEOUT: Duration = Duration::from_secs(30);
// Match the existing API's bounded envelope budget, without buffering an unbounded reply.
const MAX_READ_REPLY_BYTES: usize = 4 * collaboration_client::protocol::MAX_CONTROL_FRAME_BYTES;

pub(super) struct ConfiguredInventoryClient {
    client: reqwest::Client,
    url: String,
}

impl ConfiguredInventoryClient {
    pub(super) fn new(profile: &RouterConnectionProfile) -> Result<Self, ClientError> {
        // Credential presentation is operator-owned; never guess a header or read a secret.
        if profile.credential.is_some() {
            return Err(ClientError::InvalidRequest(
                "configured read credential presentation is unqualified",
            ));
        }
        let address = url::Url::parse(profile.mcp_address.as_str())
            .map_err(|_| ClientError::InvalidRequest("invalid configured source address"))?;
        let mut client_builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(READ_TIMEOUT);
        match address.scheme() {
            // This private client only posts to its fixed URL and never follows redirects.
            // Plain HTTP cannot use TLS roots; avoid native certificate-store IO here.
            "http" => client_builder = client_builder.tls_built_in_root_certs(false),
            "https" => {}
            _ => {
                return Err(ClientError::InvalidRequest(
                    "invalid configured source scheme",
                ));
            }
        }
        let client = client_builder
            .build()
            .map_err(|_| ClientError::Protocol("configured read transport unavailable"))?;
        Ok(Self {
            client,
            url: profile.mcp_address.as_str().to_owned(),
        })
    }

    pub(super) async fn inventory(&self) -> Result<EndpointInventory, ClientError> {
        self.call("endpoints_list", &serde_json::json!({})).await
    }

    pub(super) async fn provider_page(
        &self,
        request: &ProviderSessionListParams,
    ) -> Result<ProviderSessionListResult, ClientError> {
        self.call("provider_sessions_list", request).await
    }

    async fn call<TResult: DeserializeOwned>(
        &self,
        tool: &str,
        arguments: &impl Serialize,
    ) -> Result<TResult, ClientError> {
        let arguments = serde_json::to_value(arguments)
            .map_err(|_| ClientError::InvalidRequest("invalid source read arguments"))?;
        let serde_json::Value::Object(arguments) = arguments else {
            return Err(ClientError::InvalidRequest(
                "source read arguments must be an object",
            ));
        };
        let id = NumberOrString::Number(1);
        let request = ClientJsonRpcMessage::request(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments),
            )),
            id.clone(),
        );
        let mut response = self
            .client
            .post(&self.url)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .header("mcp-protocol-version", "2025-11-25")
            .json(&request)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    ClientError::Timeout
                } else {
                    ClientError::Transport(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "source read transport failed",
                    ))
                }
            })?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(ClientError::Rejected {
                code: i64::from(response.status().as_u16()),
                data: None,
            });
        }
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.split(';').next().is_some_and(|media_type| {
                    media_type.trim().eq_ignore_ascii_case("application/json")
                })
            })
        {
            return Err(ClientError::Protocol(
                "source does not provide stateless JSON reads",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            ClientError::Transport(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "source reply transport failed",
            ))
        })? {
            if chunk.len() > MAX_READ_REPLY_BYTES.saturating_sub(bytes.len()) {
                return Err(ClientError::Protocol("source read reply exceeded bounds"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let message: ServerJsonRpcMessage = serde_json::from_slice(&bytes)
            .map_err(|_| ClientError::Protocol("invalid source API reply"))?;
        let value = match message {
            ServerJsonRpcMessage::Response(response) if response.id == id => {
                let ServerResult::CallToolResult(result) = response.result else {
                    return Err(ClientError::Protocol("source reply was not a tool result"));
                };
                if result.is_error == Some(true) {
                    return Err(ClientError::Rejected {
                        code: -32050,
                        data: None,
                    });
                }
                result
                    .structured_content
                    .ok_or(ClientError::Protocol("source result is not structured"))?
            }
            // Failure details can contain private data. Only the fixed category crosses this edge.
            ServerJsonRpcMessage::Error(_) => {
                return Err(ClientError::Rejected {
                    code: -32050,
                    data: None,
                });
            }
            _ => {
                return Err(ClientError::Protocol(
                    "source reply did not match its request",
                ));
            }
        };
        serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid source inventory shape"))
    }
}

impl NativeInventoryReadClient for ConfiguredInventoryClient {
    async fn read_native_page(
        &mut self,
        request: NativeSessionListParams,
    ) -> Result<NativeSessionListResult, ClientError> {
        self.call("sessions_list", &request).await
    }
}

#[cfg(test)]
#[path = "configured_inventory_transport_tests.rs"]
mod tests;
