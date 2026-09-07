//! Native message composition with explicit delivery and retained partial effects.
use crate::{
    agent_declaration::render_message, message_effect_state::MessageEffects,
    native_control_dispatch::NativeControlRequest,
};
use codex_native_integration::{
    NativeConnectionError, NativeOperation, NativePayloadSchemas, NativeProtocolConnection,
};
use communication_protocol::{
    AcceptedResumeEffect, ChannelDescription, MessageDelivery, NativeInputDisposition,
    NativeInputOperation, NativeSendAcceptance, NativeSendParams, NativeSendReceipt, NonEmptyText,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(crate) async fn dispatch_message(request: NativeControlRequest<'_>) -> Value {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut effects = MessageEffects::new(request.id);
    let Ok(params) = serde_json::from_value::<NativeSendParams>(request.params) else {
        return json!({"jsonrpc":"2.0","id":effects.id,"error":{"code":-32602,"message":"Invalid message parameters"}});
    };
    if &params.target.endpoint.service_id != request.service_id {
        return effects.failure("wrongService", "inspect");
    }
    let Some(endpoint) = request
        .endpoints
        .iter()
        .find(|e| e.endpoint == params.target.endpoint)
    else {
        return effects.failure("endpointNotFound", "inspect");
    };
    let Some(backend) = request
        .backend
        .filter(|b| b.endpoint == params.target.endpoint)
    else {
        return effects.failure("unsupportedCapability", "inspect");
    };
    let Ok(admission) = backend.gate.acquire() else {
        return effects.failure("unavailable", "inspect");
    };
    if admission.generation() != &params.generation {
        return effects.failure("staleGeneration", "inspect");
    }
    let Some(schemas) = admission.schemas() else {
        return effects.failure("unsupportedCapability", "inspect");
    };
    let advertised = endpoint.channels.iter().any(|c| matches!(c, ChannelDescription::NativeCodex { schema_digest: Some(d), generation: Some(g), .. } if g == admission.generation() && String::from(d.clone()) == schemas.schema_digest()));
    if !advertised {
        return effects.failure("unsupportedCapability", "inspect");
    }
    if params.delivery == MessageDelivery::Queue
        && !schemas.supports_operation(NativeOperation::QueueAdd)
    {
        return effects.failure("unsupportedCapability", "queue");
    }
    let Ok(rendered) = render_message(&params.target, &params.message) else {
        return effects.failure("overloaded", "inspect");
    };
    let correlation = match &params.client_user_message_id {
        Some(id) => String::from(id.clone()),
        None => match crate::new_service_uuid() {
            Ok(id) => String::from(id),
            Err(_) => return effects.failure("unavailable", "inspect"),
        },
    };
    effects.correlation = Some(correlation.clone());
    let retired = admission.retirement();
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
        return effects.failure("unavailable", "inspect");
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
        .deliver(&target_id, params.delivery, &rendered.text, &correlation)
        .await;
    let acceptance = match result {
        Ok(value) => value,
        Err(value) => return value,
    };
    let Ok(correlation) = NonEmptyText::try_from(correlation) else {
        return session.effects.failure("outcomeUnknown", "start");
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
    json!({"jsonrpc":"2.0","id":session.effects.id,"result":receipt})
}

struct MessageSession {
    deadline: tokio::time::Instant,
    connection: NativeProtocolConnection,
    schemas: Arc<NativePayloadSchemas>,
    retired: CancellationToken,
    effects: MessageEffects,
}
impl MessageSession {
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
                    NativeConnectionError::Rejected { .. } => {
                        if stage == "resume" {
                            self.effects.resume = "rejected";
                        } else if mutation {
                            self.effects.submission = "rejected";
                        }
                        "nativeRejected"
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
    async fn read(&mut self, id: &str, turns: bool) -> Result<Value, Value> {
        let result = self
            .call(
                NativeOperation::ReadThread,
                json!({"threadId":id,"includeTurns":turns}),
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
    async fn deliver(
        &mut self,
        id: &str,
        delivery: MessageDelivery,
        text: &str,
        correlation: &str,
    ) -> Result<NativeSendAcceptance, Value> {
        let thread = self.read(id, false).await?;
        let status = thread
            .pointer("/status/type")
            .and_then(Value::as_str)
            .ok_or_else(|| self.effects.failure("unsupportedCapability", "inspect"))?;
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
            let observed = self.read(id, true).await?;
            let turn = observed
                .get("turns")
                .and_then(Value::as_array)
                .and_then(|turns| {
                    turns.iter().rev().find(|turn| {
                        turn.get("status").and_then(Value::as_str) == Some("inProgress")
                    })
                })
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| self.effects.failure("unsupportedCapability", "steer"))?;
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
                    json!({"threadId":id}),
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
