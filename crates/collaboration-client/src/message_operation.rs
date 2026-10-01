//! Shared message request preparation for CLI and MCP callers.
use crate::{
    ClientError, ControlClient, OperationEffect, OperationFailure,
    operation_failure_from_client_error,
};
use collaboration_protocol::{
    CodexGeneration, MessageContent, MessageDelivery, MessageText, PushMessageSendResult,
    SessionMessageReplyParams, SessionMessageReplyResult, SessionMessageSendParams, SessionRef,
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
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageReplyRequest {
    pub caller: SessionRef,
    pub reference: String,
    pub text: MessageText,
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

#[derive(Debug, thiserror::Error)]
pub enum MessageReplyError {
    #[error(transparent)]
    Preparation(Box<ClientError>),
    #[error("{source}")]
    Submission {
        caller: SessionRef,
        #[source]
        source: Box<ClientError>,
    },
}

impl MessageReplyError {
    #[must_use]
    pub fn into_operation_failure_and_caller(self) -> (OperationFailure, Option<SessionRef>) {
        match self {
            Self::Preparation(error) => (
                operation_failure_from_client_error(*error, OperationEffect::None),
                None,
            ),
            Self::Submission { caller, source } => (
                operation_failure_from_client_error(*source, OperationEffect::Unknown),
                Some(caller),
            ),
        }
    }
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
    ) -> Result<PushMessageSendResult, MessageSendError> {
        if request.target.endpoint.service_id != self.identity().service_id {
            return Err(MessageSendError::Preparation(Box::new(
                ClientError::InvalidRequest("message target belongs to another service"),
            )));
        }
        let target = request.target.clone();
        let params = SessionMessageSendParams {
            target: request.target,
            message: request.message.into(),
            mode: request.delivery,
            generation_guard: request.generation_guard,
        };
        self.submit_message_with_push(params)
            .await
            .map_err(|source| MessageSendError::Submission {
                target,
                source: Box::new(source),
            })
    }

    pub async fn reply_to_push(
        &mut self,
        request: MessageReplyRequest,
    ) -> Result<SessionMessageReplyResult, MessageReplyError> {
        let caller = request.caller.clone();
        let params = SessionMessageReplyParams {
            caller: request.caller,
            reference: request.reference,
            text: request.text,
        };
        match self.message_reply(params).await {
            Ok(receipt) => Ok(receipt),
            Err(error) if is_reply_pre_dispatch_rejection(&error) => {
                Err(MessageReplyError::Preparation(Box::new(error)))
            }
            Err(error) => Err(MessageReplyError::Submission {
                caller,
                source: Box::new(error),
            }),
        }
    }

    pub async fn router_show(
        &mut self,
        request: collaboration_protocol::PushRecordShowParams,
    ) -> Result<collaboration_protocol::PushRecordShowResult, ClientError> {
        if request.caller.endpoint.service_id != self.identity().service_id {
            return Err(ClientError::InvalidRequest(
                "push show caller belongs to another service",
            ));
        }
        let value = self
            .connection
            .call("router/show", serde_json::json!(request))
            .await?;
        serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid Router push show result"))
    }

    pub async fn message_inbox(
        &mut self,
        request: collaboration_protocol::PushRecordListParams,
    ) -> Result<collaboration_protocol::PushRecordListResult, ClientError> {
        if request.caller.endpoint.service_id != self.identity().service_id {
            return Err(ClientError::InvalidRequest(
                "message inbox caller belongs to another service",
            ));
        }
        let value = self
            .connection
            .call("message/inbox", serde_json::json!(request))
            .await?;
        serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid message inbox result"))
    }

    pub async fn message_history(
        &mut self,
        request: collaboration_protocol::PushRecordHistoryParams,
    ) -> Result<collaboration_protocol::PushRecordListResult, ClientError> {
        if request.caller.endpoint.service_id != self.identity().service_id
            || request.with.endpoint.service_id != self.identity().service_id
        {
            return Err(ClientError::InvalidRequest(
                "message history sessions must belong to this service",
            ));
        }
        let value = self
            .connection
            .call("message/history", serde_json::json!(request))
            .await?;
        serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid message history result"))
    }
}

fn is_reply_pre_dispatch_rejection(error: &ClientError) -> bool {
    match error {
        ClientError::InvalidRequest(_) => true,
        ClientError::Rejected {
            data: Some(data), ..
        } => matches!(
            data.get("kind").and_then(serde_json::Value::as_str),
            Some(
                "wrongService"
                    | "unavailable"
                    | "notFound"
                    | "notPermitted"
                    | "notDirectMessage"
                    | "ownerReplyUnsupported"
                    | "invalidField"
            )
        ),
        ClientError::Rejected { data: None, .. } => false,
        ClientError::Discovery { .. }
        | ClientError::Transport(_)
        | ClientError::Protocol(_)
        | ClientError::Timeout
        | ClientError::UnsupportedCapability(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageSendError, MessageSendRequest, PublicMessageContent};
    use crate::{ClientError, ControlClient};
    use collaboration_protocol::{
        CodexGeneration, EndpointId, EndpointRef, MessageDelivery, MessageText, SessionId,
        SessionRef, UuidIdentity,
    };
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::task::JoinHandle;

    const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
    const PUSH_ID: &str = "019f0000-0000-7000-8000-000000000101";
    const OTHER_PUSH_ID: &str = "019f0000-0000-7000-8000-000000000102";

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
                "pushId":PUSH_ID,
                "link":format!("router://{SERVICE_ID}/push/{PUSH_ID}"),
                "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},"sessionId":"target"},
                "targetIdentity":"✳️ Codex target",
                "receipt":{
                    "outcome":{"kind":"notSubmitted","retryable":false,"reason":"staleGeneration"},
                    "reachability":"codexAppServer","client":null
                }
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
            "generationGuard":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1}
        }))
        .expect("message request");
        let receipt = client
            .send_message(request)
            .await
            .expect("stale guard is an E5 result");
        assert!(matches!(
            receipt.receipt.outcome,
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
                "pushId":PUSH_ID,
                "link":format!("router://{SERVICE_ID}/push/{PUSH_ID}"),
                "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},"sessionId":"target"},
                "targetIdentity":"✳️ Codex target",
                "receipt":{
                    "outcome":{"kind":"notSubmitted","retryable":true,"reason":"provider starting"},
                    "reachability":null,"client":null
                }
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
            Ok(collaboration_protocol::PushMessageSendResult {
                receipt: collaboration_protocol::DeliveryReceipt {
                    outcome: collaboration_protocol::DeliveryOutcome::NotSubmitted {
                        retryable: true,
                        ..
                    },
                    reachability: None,
                    client: None
                },
                ..
            })
        ));
        peer.await.expect("join peer");
    }

    #[tokio::test]
    async fn unavailable_after_message_submission_keeps_outcome_unknown() {
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
            let response = json!({"jsonrpc":"2.0","id":sent["id"],"error":{
                "code":-32050,"message":"Native backend unavailable",
                "data":{"kind":"unavailable","stage":"start","message":"Native backend unavailable"}
            }});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write unavailable response");
            assert!(lines.next_line().await.expect("read close").is_none());
        });
        let mut client = ControlClient::initialize(client_stream, "message-test", "1")
            .await
            .expect("initialize");
        let request = fixture_request(None);
        let expected_target = request.target.clone();
        let error = client
            .send_message(request)
            .await
            .expect_err("post-submission unavailable may hide an accepted delivery");
        assert!(matches!(
            error,
            MessageSendError::Submission { target, source }
                if target == expected_target
                    && matches!(source.as_ref(), ClientError::Rejected { data: Some(data), .. }
                        if data["kind"] == "unavailable")
        ));
        client.close().await.expect("close client");
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

    #[tokio::test]
    async fn successful_push_response_must_name_the_requested_target() {
        let request = fixture_request(None);
        let mut response = push_result(&request.target, PUSH_ID, PUSH_ID);
        response["target"]["sessionId"] = json!("different-target");
        let (mut client, peer) = control_client_with_send_result(response).await;

        let error = client
            .send_message(request)
            .await
            .expect_err("a successful receipt for another target is inconsistent");
        assert_protocol_inconsistency(error);

        client.close().await.expect("close client");
        peer.await.expect("join peer");
    }

    #[tokio::test]
    async fn successful_push_response_link_must_match_its_push_id() {
        let request = fixture_request(None);
        let response = push_result(&request.target, PUSH_ID, OTHER_PUSH_ID);
        let (mut client, peer) = control_client_with_send_result(response).await;

        let error = client
            .send_message(request)
            .await
            .expect_err("a link to another push is inconsistent");
        assert_protocol_inconsistency(error);

        client.close().await.expect("close client");
        peer.await.expect("join peer");
    }

    #[tokio::test]
    async fn native_push_receipt_matches_target_generation_mode_and_push_id() {
        let expected_generation = generation(1);
        let request = fixture_request(Some(expected_generation.clone()));
        let response = native_push_result(
            &request.target,
            &request.target,
            &expected_generation,
            PUSH_ID,
            PUSH_ID,
            "started",
        );
        let (mut client, peer) = control_client_with_send_result(response).await;

        let result = client
            .send_message(request)
            .await
            .expect("matching native delivery receipt");
        assert_eq!(result.push_id.as_str(), PUSH_ID);
        client.close().await.expect("close client");
        peer.await.expect("join peer");

        let request = fixture_request(Some(expected_generation.clone()));
        let other_target = session_ref("different-target");
        let response = native_push_result(
            &request.target,
            &other_target,
            &expected_generation,
            PUSH_ID,
            PUSH_ID,
            "started",
        );
        assert_inconsistent_response(request, response).await;

        let request = fixture_request(Some(expected_generation.clone()));
        let response = native_push_result(
            &request.target,
            &request.target,
            &generation(2),
            PUSH_ID,
            PUSH_ID,
            "started",
        );
        assert_inconsistent_response(request, response).await;

        let mut request = fixture_request(Some(expected_generation.clone()));
        request.delivery = MessageDelivery::Queue;
        let response = native_push_result(
            &request.target,
            &request.target,
            &expected_generation,
            PUSH_ID,
            PUSH_ID,
            "started",
        );
        assert_inconsistent_response(request, response).await;

        let request = fixture_request(Some(expected_generation.clone()));
        let response = native_push_result(
            &request.target,
            &request.target,
            &expected_generation,
            PUSH_ID,
            OTHER_PUSH_ID,
            "started",
        );
        assert_inconsistent_response(request, response).await;
    }

    fn fixture_request(
        generation_guard: Option<collaboration_protocol::CodexGeneration>,
    ) -> MessageSendRequest {
        serde_json::from_value(json!({
            "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"target"},
            "message":{"kind":"humanUser","text":"hello"},"delivery":"auto",
            "generationGuard":generation_guard
        }))
        .expect("message request")
    }

    async fn control_client_with_send_result(result: Value) -> (ControlClient, JoinHandle<()>) {
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
            let initialize_result = json!({
                "jsonrpc":"2.0",
                "id":initialize["id"],
                "result":{
                    "version":{"major":1,"minor":0},
                    "serviceId":SERVICE_ID,
                    "serviceEpoch":"00000000-0000-4000-8000-000000000002",
                    "controlSchemaDigest":format!("sha256:{}", "a".repeat(64))
                }
            });
            write
                .write_all(format!("{initialize_result}\n").as_bytes())
                .await
                .expect("write initialization result");

            let send: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read message send")
                    .expect("message send frame"),
            )
            .expect("message send JSON");
            assert_eq!(send["method"], "message/send");
            let response = json!({"jsonrpc":"2.0","id":send["id"],"result":result});
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("write push result");
            assert!(lines.next_line().await.expect("read close").is_none());
        });
        let client = ControlClient::initialize(client_stream, "message-test", "1")
            .await
            .expect("initialize");
        (client, peer)
    }

    fn push_result(target: &SessionRef, push_id: &str, link_push_id: &str) -> Value {
        json!({
            "pushId":push_id,
            "link":format!("router://{SERVICE_ID}/push/{link_push_id}"),
            "target":target,
            "targetIdentity":"✳️ Codex target",
            "receipt":{
                "outcome":{"kind":"peerMessageWritten"},
                "reachability":"claudeCodePeer",
                "client":{"kind":"claudeCodePeer"}
            }
        })
    }

    fn native_push_result(
        target: &SessionRef,
        native_target: &SessionRef,
        generation: &CodexGeneration,
        push_id: &str,
        client_user_message_id: &str,
        outcome: &str,
    ) -> Value {
        json!({
            "pushId":push_id,
            "link":format!("router://{SERVICE_ID}/push/{push_id}"),
            "target":target,
            "targetIdentity":"✳️ Codex target",
            "receipt":{
                "outcome":{"kind":outcome},
                "reachability":"codexAppServer",
                "client":{
                    "kind":"codexAppServer",
                    "target":native_target,
                    "generation":generation,
                    "inputKind":"agent",
                    "representation":"declaredAgentText",
                    "clientUserMessageId":client_user_message_id,
                    "resumeEffect":"notRequested",
                    "acceptance":{
                        "kind":"nativeInputAccepted",
                        "operation":"turnStart",
                        "disposition":"startedOrSteered",
                        "turnId":"turn-1"
                    }
                }
            }
        })
    }

    fn generation(generation: u64) -> CodexGeneration {
        serde_json::from_value(json!({
            "serviceEpoch":"00000000-0000-4000-8000-000000000002",
            "generation":generation
        }))
        .expect("Codex generation")
    }

    fn session_ref(session_id: &str) -> SessionRef {
        serde_json::from_value(json!({
            "endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},
            "sessionId":session_id
        }))
        .expect("session reference")
    }

    fn assert_protocol_inconsistency(error: MessageSendError) {
        assert!(matches!(
            error,
            MessageSendError::Submission { source, .. }
                if matches!(source.as_ref(), ClientError::Protocol(
                    "inconsistent message receipt; acceptance unknown"
                ))
        ));
    }

    async fn assert_inconsistent_response(request: MessageSendRequest, response: Value) {
        let (mut client, peer) = control_client_with_send_result(response).await;
        let error = client
            .send_message(request)
            .await
            .expect_err("inconsistent delivery data must fail closed");
        assert_protocol_inconsistency(error);
        client.close().await.expect("close client");
        peer.await.expect("join peer");
    }
}
