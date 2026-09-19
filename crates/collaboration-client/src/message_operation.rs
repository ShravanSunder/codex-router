//! Shared message preparation and generation discovery for CLI and MCP callers.
use crate::{ClientError, ControlClient, OperationEffect, OperationFailure};
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, MessageContent, MessageDelivery, NativeSendParams,
    NativeSendReceipt, NonEmptyText, SessionRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSendRequest {
    pub target: SessionRef,
    pub message: PublicMessageContent,
    #[serde(default)]
    pub delivery: MessageDelivery,
    pub generation_guard: Option<CodexGeneration>,
    pub client_user_message_id: Option<NonEmptyText>,
}

#[derive(Debug, thiserror::Error)]
pub enum MessageSendError {
    #[error(transparent)]
    Preparation(ClientError),
    #[error(transparent)]
    Submission(ClientError),
}

impl MessageSendError {
    #[must_use]
    pub fn into_operation_failure(self) -> OperationFailure {
        match self {
            Self::Preparation(error) => {
                OperationFailure::from_client_error(error, OperationEffect::None)
            }
            Self::Submission(error) => {
                OperationFailure::from_client_error(error, OperationEffect::Unknown)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PublicMessageContent {
    Agent {
        sender: SessionRef,
        text: collaboration_protocol::MessageText,
    },
    HumanUser {
        text: collaboration_protocol::MessageText,
    },
}

impl TryFrom<MessageContent> for PublicMessageContent {
    type Error = ClientError;
    fn try_from(value: MessageContent) -> Result<Self, Self::Error> {
        match value {
            MessageContent::Agent { sender, text } => Ok(Self::Agent { sender, text }),
            MessageContent::HumanUser { text } => Ok(Self::HumanUser { text }),
            MessageContent::Router { .. } => Err(ClientError::InvalidRequest(
                "Router message content is internal-only",
            )),
        }
    }
}

impl From<PublicMessageContent> for MessageContent {
    fn from(value: PublicMessageContent) -> Self {
        match value {
            PublicMessageContent::Agent { sender, text } => Self::Agent { sender, text },
            PublicMessageContent::HumanUser { text } => Self::HumanUser { text },
        }
    }
}

impl ControlClient {
    pub async fn send_message(
        &mut self,
        request: MessageSendRequest,
    ) -> Result<NativeSendReceipt, MessageSendError> {
        if request.target.endpoint.service_id != self.identity().service_id {
            return Err(MessageSendError::Preparation(ClientError::InvalidRequest(
                "message target belongs to another service",
            )));
        }
        let inventory = self
            .list_endpoints()
            .await
            .map_err(MessageSendError::Preparation)?;
        let generation = inventory
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint == request.target.endpoint)
            .and_then(|endpoint| {
                endpoint.channels.iter().find_map(|channel| match channel {
                    ChannelDescription::NativeCodex {
                        generation: Some(generation),
                        ..
                    } => Some(generation.clone()),
                    _ => None,
                })
            })
            .ok_or(MessageSendError::Preparation(ClientError::Rejected {
                code: -32050,
                data: Some(json!({"kind":"unavailable","stage":"discovery"})),
            }))?;
        if request
            .generation_guard
            .as_ref()
            .is_some_and(|expected| expected != &generation)
        {
            return Err(MessageSendError::Preparation(ClientError::Rejected {
                code: -32050,
                data: Some(json!({"kind":"staleGeneration","stage":"discovery"})),
            }));
        }
        let is_agent = matches!(request.message, PublicMessageContent::Agent { .. });
        let params = NativeSendParams {
            target: request.target,
            generation,
            message: request.message.into(),
            delivery: request.delivery,
            client_user_message_id: request.client_user_message_id,
        };
        if is_agent {
            self.send_agent_message(params)
                .await
                .map_err(MessageSendError::Submission)
        } else {
            self.send_human_input(params)
                .await
                .map_err(MessageSendError::Submission)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageSendError, MessageSendRequest, PublicMessageContent};
    use crate::{ClientError, ControlClient};
    use collaboration_protocol::{
        EndpointId, EndpointRef, MessageDelivery, MessageText, SessionId, SessionRef, UuidIdentity,
    };
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn wrong_service_is_rejected_before_endpoint_discovery_or_submission() {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("stream pair");
        let peer = tokio::spawn(async move {
            let (read, mut write) = server_stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let initialize: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read init")
                    .expect("init frame"),
            )
            .expect("init JSON");
            let response = json!({"jsonrpc":"2.0","id":initialize["id"],"result":{
                "version":{"major":1,"minor":0},
                "serviceId":"00000000-0000-4000-8000-000000000001",
                "serviceEpoch":"00000000-0000-4000-8000-000000000002",
                "controlSchemaDigest":format!("sha256:{}", "a".repeat(64))
            }});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write init");
            assert!(lines.next_line().await.expect("read close").is_none());
        });
        let mut client = ControlClient::initialize(client_stream, "message-test", "1")
            .await
            .expect("initialize");
        let request = MessageSendRequest {
            target: SessionRef {
                endpoint: EndpointRef {
                    service_id: UuidIdentity::try_from(
                        "00000000-0000-4000-8000-000000000099".to_owned(),
                    )
                    .expect("service id"),
                    endpoint_id: EndpointId::try_from("codex-local".to_owned())
                        .expect("endpoint id"),
                },
                session_id: SessionId::try_from("target".to_owned()).expect("session id"),
            },
            message: PublicMessageContent::HumanUser {
                text: MessageText::try_from("hello".to_owned()).expect("message text"),
            },
            delivery: MessageDelivery::Auto,
            generation_guard: None,
            client_user_message_id: None,
        };
        assert!(matches!(
            client.send_message(request).await,
            Err(MessageSendError::Preparation(ClientError::InvalidRequest(
                "message target belongs to another service"
            )))
        ));
        client.close().await.expect("close client");
        peer.await.expect("join peer");
    }

    #[tokio::test]
    async fn stale_generation_guard_is_rejected_after_discovery_without_submission() {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("stream pair");
        let peer = tokio::spawn(async move {
            let (read, mut write) = server_stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let initialize: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read init")
                    .expect("init frame"),
            )
            .expect("init JSON");
            let response = json!({"jsonrpc":"2.0","id":initialize["id"],"result":{
                "version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001",
                "serviceEpoch":"00000000-0000-4000-8000-000000000002","controlSchemaDigest":format!("sha256:{}", "a".repeat(64))
            }});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write init");
            let inventory: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read inventory")
                    .expect("inventory frame"),
            )
            .expect("inventory JSON");
            assert_eq!(inventory["method"], "endpoint/list");
            let response = json!({"jsonrpc":"2.0","id":inventory["id"],"result":{
                "serviceEpoch":"00000000-0000-4000-8000-000000000002","sequence":1,
                "endpoints":[{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
                    "label":"fixture","availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
                    "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,
                        "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":2}}]}]
            }});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write inventory");
            assert!(lines.next_line().await.expect("read close").is_none());
        });
        let mut client = ControlClient::initialize(client_stream, "message-test", "1")
            .await
            .expect("initialize");
        let request: MessageSendRequest = serde_json::from_value(json!({
            "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"target"},
            "message":{"kind":"humanUser","text":"hello"},"delivery":"auto",
            "generationGuard":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},
            "clientUserMessageId":null
        }))
        .expect("message request");
        assert!(matches!(
            client.send_message(request).await,
            Err(MessageSendError::Preparation(ClientError::Rejected { code: -32050, data: Some(data) }))
                if data["kind"] == "staleGeneration"
        ));
        client.close().await.expect("close client");
        peer.await.expect("join peer");
    }

    #[tokio::test]
    async fn discovery_transport_loss_is_preparation_failure_without_submission() {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("stream pair");
        let peer = tokio::spawn(async move {
            let (read, mut write) = server_stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let initialize: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read init")
                    .expect("init frame"),
            )
            .expect("init JSON");
            let response = json!({"jsonrpc":"2.0","id":initialize["id"],"result":{
                "version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001",
                "serviceEpoch":"00000000-0000-4000-8000-000000000002","controlSchemaDigest":format!("sha256:{}", "a".repeat(64))
            }});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write init");
            let discovery: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read discovery")
                    .expect("discovery frame"),
            )
            .expect("discovery JSON");
            assert_eq!(discovery["method"], "endpoint/list");
        });
        let mut client = ControlClient::initialize(client_stream, "message-test", "1")
            .await
            .expect("initialize");
        let request = fixture_request(None);
        assert!(matches!(
            client.send_message(request).await,
            Err(super::MessageSendError::Preparation(_))
        ));
        peer.await.expect("join peer");
    }

    #[tokio::test]
    async fn response_loss_after_message_dispatch_is_submission_failure() {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("stream pair");
        let peer = tokio::spawn(async move {
            let (read, mut write) = server_stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let initialize: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read init")
                    .expect("init frame"),
            )
            .expect("init JSON");
            let response = json!({"jsonrpc":"2.0","id":initialize["id"],"result":{
                "version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001",
                "serviceEpoch":"00000000-0000-4000-8000-000000000002","controlSchemaDigest":format!("sha256:{}", "a".repeat(64))
            }});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write init");
            let discovery: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read discovery")
                    .expect("discovery frame"),
            )
            .expect("discovery JSON");
            let response = json!({"jsonrpc":"2.0","id":discovery["id"],"result":{
                "serviceEpoch":"00000000-0000-4000-8000-000000000002","sequence":1,
                "endpoints":[{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
                    "label":"fixture","availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
                    "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,
                        "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":2}}]}]
            }});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write discovery");
            let send: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read send")
                    .expect("send frame"),
            )
            .expect("send JSON");
            assert_eq!(send["method"], "codex/messageSend");
        });
        let mut client = ControlClient::initialize(client_stream, "message-test", "1")
            .await
            .expect("initialize");
        assert!(matches!(
            client.send_message(fixture_request(None)).await,
            Err(super::MessageSendError::Submission(_))
        ));
        peer.await.expect("join peer");
    }

    fn fixture_request(
        generation_guard: Option<collaboration_protocol::CodexGeneration>,
    ) -> MessageSendRequest {
        serde_json::from_value(json!({
            "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"target"},
            "message":{"kind":"humanUser","text":"hello"},"delivery":"auto",
            "generationGuard":generation_guard,"clientUserMessageId":null
        }))
        .expect("message request")
    }
}
