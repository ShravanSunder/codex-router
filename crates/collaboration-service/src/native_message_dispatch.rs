//! Native message composition with explicit delivery and retained partial effects.
use crate::{NativeControlBackend, message_effect_state::MessageEffects};
use codex_native_integration::{
    NativeConnectionError, NativeOperation, NativePayloadSchemas, NativeProtocolConnection,
};
use collaboration_protocol::{
    AcceptedResumeEffect, ChannelDescription, EndpointDescription, MessageDelivery,
    NativeInputDisposition, NativeInputOperation, NativeSendAcceptance, NativeSendParams,
    NativeSendReceipt, NonEmptyText, UuidIdentity,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(crate) struct NativeMessageRequest<'a> {
    pub params: NativeSendParams,
    pub id: Value,
    pub service_id: &'a UuidIdentity,
    pub backend: &'a NativeControlBackend,
    pub endpoints: &'a [EndpointDescription],
    pub held_connection: Option<&'a mut NativeProtocolConnection>,
}

pub(crate) enum NativeMessageOutcome {
    Accepted(NativeSendReceipt),
    Failed(Value),
}

pub(crate) async fn dispatch_message(
    mut request: NativeMessageRequest<'_>,
) -> NativeMessageOutcome {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut effects = MessageEffects::new(request.id);
    let params = request.params;
    if &params.target.endpoint.service_id != request.service_id {
        return NativeMessageOutcome::Failed(effects.failure("wrongService", "inspect"));
    }
    let Some(endpoint) = request
        .endpoints
        .iter()
        .find(|e| e.endpoint == params.target.endpoint)
    else {
        return NativeMessageOutcome::Failed(effects.failure("endpointNotFound", "inspect"));
    };
    let backend = request.backend;
    if backend.endpoint != params.target.endpoint {
        return NativeMessageOutcome::Failed(effects.failure("unsupportedCapability", "inspect"));
    }
    let Ok(admission) = backend.gate.acquire() else {
        return NativeMessageOutcome::Failed(effects.failure("unavailable", "inspect"));
    };
    if admission.generation() != &params.generation {
        return NativeMessageOutcome::Failed(effects.failure("staleGeneration", "inspect"));
    }
    let Some(schemas) = admission.schemas() else {
        return NativeMessageOutcome::Failed(effects.failure("unsupportedCapability", "inspect"));
    };
    let advertised = endpoint.channels.iter().any(|c| matches!(c, ChannelDescription::NativeCodex { schema_digest: Some(d), generation: Some(g), .. } if g == admission.generation() && String::from(d.clone()) == schemas.schema_digest()));
    if !advertised {
        return NativeMessageOutcome::Failed(effects.failure("unsupportedCapability", "inspect"));
    }
    if params.delivery == MessageDelivery::Queue
        && !schemas.supports_operation(NativeOperation::QueueAdd)
    {
        return NativeMessageOutcome::Failed(effects.failure("unsupportedCapability", "queue"));
    }
    let Ok(rendered) = collaboration_protocol::render_message(&params.target, &params.message)
    else {
        return NativeMessageOutcome::Failed(effects.failure("overloaded", "inspect"));
    };
    let correlation = match &params.client_user_message_id {
        Some(id) => String::from(id.clone()),
        None => match crate::new_service_uuid() {
            Ok(id) => String::from(id),
            Err(_) => {
                return NativeMessageOutcome::Failed(effects.failure("unavailable", "inspect"));
            }
        },
    };
    effects.correlation = Some(correlation.clone());
    let retired = admission.retirement();
    let held = request.held_connection.is_some();
    let mut opened_connection = None;
    if !held {
        let connection = tokio::time::timeout_at(deadline, async {
            tokio::select! {
                biased;
                _ = retired.cancelled() => Err(NativeConnectionError::Unavailable),
                result = NativeProtocolConnection::connect(admission.backend_path()) => result,
            }
        })
        .await
        .unwrap_or(Err(NativeConnectionError::Unavailable));
        let Ok(connection) = connection else {
            return NativeMessageOutcome::Failed(effects.failure("unavailable", "inspect"));
        };
        opened_connection = Some(connection);
    }
    let connection = match request.held_connection.take() {
        Some(connection) => connection,
        None => match opened_connection.as_mut() {
            Some(connection) => connection,
            None => return NativeMessageOutcome::Failed(effects.failure("unavailable", "inspect")),
        },
    };
    let mut session = MessageSession {
        deadline,
        connection,
        schemas,
        retired,
        effects,
    };
    let target_id = String::from(params.target.session_id.clone());
    let result = session
        .deliver(
            &target_id,
            params.delivery,
            &rendered.text,
            &correlation,
            held,
        )
        .await;
    let acceptance = match result {
        Ok(value) => value,
        Err(value) => return NativeMessageOutcome::Failed(value),
    };
    let Ok(correlation) = NonEmptyText::try_from(correlation) else {
        return NativeMessageOutcome::Failed(session.effects.failure("outcomeUnknown", "start"));
    };
    let receipt = NativeSendReceipt {
        target: params.target,
        generation: params.generation,
        input_kind: rendered.kind,
        representation: rendered.representation,
        client_user_message_id: correlation,
        resume_effect: if session.effects.resume == "accepted" {
            AcceptedResumeEffect::Accepted
        } else {
            AcceptedResumeEffect::NotRequested
        },
        acceptance,
    };
    NativeMessageOutcome::Accepted(receipt)
}

struct MessageSession<'a> {
    deadline: tokio::time::Instant,
    connection: &'a mut NativeProtocolConnection,
    schemas: Arc<NativePayloadSchemas>,
    retired: CancellationToken,
    effects: MessageEffects,
}
impl MessageSession<'_> {
    async fn call(
        &mut self,
        operation: NativeOperation,
        params: Value,
        stage: &'static str,
    ) -> Result<Value, Value> {
        if self.retired.is_cancelled() || tokio::time::Instant::now() >= self.deadline {
            return Err(self.effects.failure("unavailable", stage));
        }
        let mutation = matches!(stage, "resume" | "start" | "steer" | "queue");
        let missing_thread_message = (operation == NativeOperation::ReadThread)
            .then(|| params.get("threadId").and_then(Value::as_str))
            .flatten()
            .map(|id| format!("thread not loaded: {id}"));
        if stage == "resume" {
            self.effects.resume = "unknown";
        } else if mutation {
            self.effects.submission = "unknown";
        }
        let result = tokio::time::timeout_at(self.deadline, async { tokio::select! {
            biased;
            result = self.connection.request_validated(&self.schemas, operation, params) => result,
            _ = self.retired.cancelled() => Err(NativeConnectionError::OutcomeUnknown),
        }}).await.unwrap_or(Err(NativeConnectionError::OutcomeUnknown));
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                let kind = match error {
                    NativeConnectionError::Rejected { code } => {
                        if stage == "resume" {
                            self.effects.resume = "rejected";
                        } else if mutation {
                            self.effects.submission = "rejected";
                        }
                        let native = self.connection.take_last_rejection();
                        if let Some(expected) = missing_thread_message.as_deref()
                            && code == -32600
                            && native
                                .as_ref()
                                .and_then(|error| error.get("message"))
                                .and_then(Value::as_str)
                                == Some(expected)
                        {
                            return Err(self
                                .effects
                                .failure("threadMissingOrUnmaterialized", stage));
                        }
                        return Err(self.effects.native_rejection(stage, code, native.as_ref()));
                    }
                    NativeConnectionError::InvalidInput | NativeConnectionError::Unavailable => {
                        if stage == "resume" {
                            self.effects.resume = "notRequested";
                        } else if mutation {
                            self.effects.submission = "notDispatched";
                        }
                        "unsupportedCapability"
                    }
                    _ if mutation => "outcomeUnknown",
                    _ => "unavailable",
                };
                Err(self.effects.failure(kind, stage))
            }
        }
    }
    async fn read_thread_metadata(&mut self, id: &str) -> Result<Value, Value> {
        let result = self
            .call(
                NativeOperation::ReadThread,
                json!({"threadId":id,"includeTurns":false}),
                "inspect",
            )
            .await?;
        if result.pointer("/thread/id").and_then(Value::as_str) != Some(id) {
            return Err(self.effects.failure("nativeRejected", "inspect"));
        }
        result
            .get("thread")
            .cloned()
            .ok_or_else(|| self.effects.failure("nativeRejected", "inspect"))
    }
    async fn read_active_turn_id(&mut self, id: &str) -> Result<String, Value> {
        let result = self
            .call(
                NativeOperation::ListTurns,
                json!({
                    "threadId":id,
                    "limit":1,
                    "sortDirection":"desc",
                    "itemsView":"notLoaded"
                }),
                "inspect",
            )
            .await?;
        result
            .get("data")
            .and_then(Value::as_array)
            .and_then(|turns| turns.first())
            .filter(|turn| turn.get("status").and_then(Value::as_str) == Some("inProgress"))
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| self.effects.failure("unsupportedCapability", "steer"))
    }
    async fn deliver(
        &mut self,
        id: &str,
        delivery: MessageDelivery,
        text: &str,
        correlation: &str,
        held_unmaterialized: bool,
    ) -> Result<NativeSendAcceptance, Value> {
        let thread = if held_unmaterialized {
            None
        } else {
            Some(self.read_thread_metadata(id).await?)
        };
        let status = match thread.as_ref() {
            None => "idle",
            Some(thread) => thread
                .pointer("/status/type")
                .and_then(Value::as_str)
                .ok_or_else(|| self.effects.failure("unsupportedCapability", "inspect"))?,
        };
        let input = json!([{"type":"text","text":text}]);
        if delivery == MessageDelivery::Queue {
            if status == "notLoaded" {
                return Err(self.effects.failure("threadNotLoaded", "queue"));
            }
            let result = self
                .call(
                    NativeOperation::QueueAdd,
                    json!({"threadId":id,"input":input,"clientUserMessageId":correlation}),
                    "queue",
                )
                .await?;
            let submission_id = self.receipt_id(&result, "/queuedSubmission/id", "queue")?;
            return Ok(NativeSendAcceptance::QueueAccepted { submission_id });
        }
        if status == "active" {
            let turn = self.read_active_turn_id(id).await?;
            let result = self.call(NativeOperation::SteerTurn, json!({"threadId":id,"expectedTurnId":turn,"input":input,"clientUserMessageId":correlation}), "steer").await?;
            let turn_id = self.receipt_id(&result, "/turnId", "steer")?;
            if String::from(turn_id.clone()) != turn {
                return Err(self.effects.failure("outcomeUnknown", "steer"));
            }
            return Ok(NativeSendAcceptance::SteerAccepted {
                turn_id,
                submission_id: None,
            });
        }
        if delivery == MessageDelivery::Steer {
            return Err(self.effects.failure("noActiveTurn", "steer"));
        }
        if status == "notLoaded" {
            let resumed = self
                .call(
                    NativeOperation::ResumeThread,
                    json!({"threadId":id,"excludeTurns":true}),
                    "resume",
                )
                .await?;
            if resumed.pointer("/thread/id").and_then(Value::as_str) != Some(id) {
                return Err(self.effects.failure("outcomeUnknown", "resume"));
            }
            self.effects.resume = "accepted";
        }
        let result = self
            .call(
                NativeOperation::StartTurn,
                json!({"threadId":id,"input":input,"clientUserMessageId":correlation}),
                "start",
            )
            .await?;
        Ok(NativeSendAcceptance::NativeInputAccepted {
            operation: NativeInputOperation::TurnStart,
            disposition: NativeInputDisposition::StartedOrSteered,
            turn_id: self.receipt_id(&result, "/turn/id", "start")?,
        })
    }
    fn receipt_id(
        &self,
        result: &Value,
        pointer: &str,
        stage: &str,
    ) -> Result<NonEmptyText, Value> {
        result
            .pointer(pointer)
            .and_then(Value::as_str)
            .and_then(|id| NonEmptyText::try_from(id.to_owned()).ok())
            .ok_or_else(|| self.effects.failure("outcomeUnknown", stage))
    }
}
