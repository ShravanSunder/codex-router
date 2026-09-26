//! Shared message request preparation for CLI and MCP callers.
use crate::{
    ClientError, ControlClient, OperationEffect, OperationFailure,
    operation_failure_from_client_error,
};
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryReceipt, MessageContent, MessageDelivery,
    SessionMessageSendParams, SessionRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSendRequest {
    pub target: SessionRef,
    pub message: PublicMessageContent,
    #[serde(default)]
    pub delivery: MessageDelivery,
    pub generation_guard: Option<CodexGeneration>,
    pub correlation: Option<DeliveryCorrelationId>,
}

#[derive(Debug, thiserror::Error)]
pub enum MessageSendError {
    #[error(transparent)]
    Preparation(Box<ClientError>),
    #[error("{source}")]
    Submission {
        target: SessionRef,
        #[source]
        source: Box<ClientError>,
    },
}

impl MessageSendError {
    #[must_use]
    pub fn into_operation_failure_and_target(self) -> (OperationFailure, Option<SessionRef>) {
        match self {
            Self::Preparation(error) => (
                operation_failure_from_client_error(*error, OperationEffect::None),
                None,
            ),
            Self::Submission { target, source } => (
                operation_failure_from_client_error(*source, OperationEffect::Unknown),
                Some(target),
            ),
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
    ) -> Result<DeliveryReceipt, MessageSendError> {
        if request.target.endpoint.service_id != self.identity().service_id {
            return Err(MessageSendError::Preparation(Box::new(
                ClientError::InvalidRequest("message target belongs to another service"),
            )));
        }
        let is_agent = matches!(request.message, PublicMessageContent::Agent { .. });
        let target = request.target.clone();
        let params = SessionMessageSendParams {
            target: request.target,
            message: request.message.into(),
            mode: request.delivery,
            generation_guard: request.generation_guard,
            correlation: request.correlation,
        };
        if is_agent {
            self.send_agent_message(params)
                .await
                .map_err(|source| MessageSendError::Submission {
                    target: target.clone(),
                    source: Box::new(source),
                })
        } else {
            self.send_human_input(params)
                .await
                .map_err(|source| MessageSendError::Submission {
                    target,
                    source: Box::new(source),
                })
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
            correlation: None,
        };
        let error = client
            .send_message(request)
            .await
            .expect_err("foreign service");
        assert!(matches!(
            error,
            MessageSendError::Preparation(source)
                if matches!(source.as_ref(), ClientError::InvalidRequest(
                    "message target belongs to another service"
                ))
        ));
        client.close().await.expect("close client");
        peer.await.expect("join peer");
    }

    #[tokio::test]
    async fn strict_generation_guard_reaches_the_service_and_reports_known_none() {
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
            let sent: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read message")
                    .expect("message frame"),
            )
            .expect("message JSON");
            assert_eq!(sent["method"], "message/send");
            assert_eq!(sent["params"]["generationGuard"]["generation"], 1);
            let response = json!({"jsonrpc":"2.0","id":sent["id"],"result":{
                "outcome":{"kind":"notSubmitted","retryable":false,"reason":"staleGeneration"},
                "reachability":"codexAppServer","client":null
            }});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write receipt");
            assert!(lines.next_line().await.expect("read close").is_none());
        });
        let mut client = ControlClient::initialize(client_stream, "message-test", "1")
            .await
            .expect("initialize");
        let request: MessageSendRequest = serde_json::from_value(json!({
            "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"target"},
            "message":{"kind":"humanUser","text":"hello"},"delivery":"auto",
            "generationGuard":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},
            "correlation":null
        }))
        .expect("message request");
        let receipt = client
            .send_message(request)
            .await
            .expect("stale guard is an E5 result");
        assert!(matches!(
            receipt.outcome,
            collaboration_protocol::DeliveryOutcome::NotSubmitted { retryable: false, reason }
                if reason == "staleGeneration"
        ));
        client.close().await.expect("close client");
        peer.await.expect("join peer");
    }

    #[tokio::test]
    async fn unpinned_send_uses_service_route_without_native_catalog_precheck() {
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
            let sent: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read message")
                    .expect("message frame"),
            )
            .expect("message JSON");
            assert_eq!(sent["method"], "message/send");
            assert!(sent["params"]["generationGuard"].is_null());
            let response = json!({"jsonrpc":"2.0","id":sent["id"],"result":{
                "outcome":{"kind":"notSubmitted","retryable":true,"reason":"provider starting"},
                "reachability":null,"client":null
            }});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write receipt");
        });
        let mut client = ControlClient::initialize(client_stream, "message-test", "1")
            .await
            .expect("initialize");
        let request = fixture_request(None);
        assert!(matches!(
            client.send_message(request).await,
            Ok(collaboration_protocol::DeliveryReceipt {
                outcome: collaboration_protocol::DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    ..
                },
                reachability: None,
                client: None
            })
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
            let send: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read send")
                    .expect("send frame"),
            )
            .expect("send JSON");
            assert_eq!(send["method"], "message/send");
        });
        let mut client = ControlClient::initialize(client_stream, "message-test", "1")
            .await
            .expect("initialize");
        let error = client
            .send_message(fixture_request(None))
            .await
            .expect_err("response loss after dispatch must fail");
        let (failure, target) = error.into_operation_failure_and_target();
        assert_eq!(failure.effect, crate::OperationEffect::Unknown);
        assert_eq!(target, Some(fixture_request(None).target));
        peer.await.expect("join peer");
    }

    fn fixture_request(
        generation_guard: Option<collaboration_protocol::CodexGeneration>,
    ) -> MessageSendRequest {
        serde_json::from_value(json!({
            "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"target"},
            "message":{"kind":"humanUser","text":"hello"},"delivery":"auto",
            "generationGuard":generation_guard,"correlation":null
        }))
        .expect("message request")
    }
}
