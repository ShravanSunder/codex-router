//! Native message composition with explicit delivery and retained partial effects.
use crate::{
    LoadPolicy, NOT_LOADED_REASON, NativeControlBackend, message_effect_state::MessageEffects,
};
use codex_native_integration::{
    NativeConnectionError, NativeOperation, NativePayloadSchemas, NativeProtocolConnection,
};
use collaboration_protocol::{
    AcceptedResumeEffect, ChannelDescription, EndpointDescription, MessageDelivery,
    MessageHeaderContext, NativeInputDisposition, NativeInputOperation, NativeSendAcceptance,
    NativeSendParams, NativeSendReceipt, NonEmptyText, SessionRef, UuidIdentity,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeThreadStatus {
    NotLoaded,
    Idle,
    Active,
    Other,
}

pub(crate) struct NativeThreadSnapshot {
    pub status: NativeThreadStatus,
    pub thread: Value,
}

pub(crate) enum NativeThreadStatusReadError {
    Missing,
    Rejected { code: i64, native: Option<Value> },
    Unsupported,
    Unavailable,
    InvalidResponse,
}

pub(crate) async fn read_native_thread_status(
    connection: &mut NativeProtocolConnection,
    schemas: &NativePayloadSchemas,
    thread_id: &str,
    deadline: tokio::time::Instant,
    retired: &CancellationToken,
) -> Result<NativeThreadSnapshot, NativeThreadStatusReadError> {
    if retired.is_cancelled() || tokio::time::Instant::now() >= deadline {
        return Err(NativeThreadStatusReadError::Unavailable);
    }
    let response = tokio::select! {
        biased;
        () = retired.cancelled() => {
            return Err(NativeThreadStatusReadError::Unavailable);
        }
        response = tokio::time::timeout_at(
            deadline,
            connection.request_validated(
                schemas,
                NativeOperation::ReadThread,
                json!({"threadId":thread_id,"includeTurns":false}),
            ),
        ) => match response {
            Ok(response) => response,
            Err(_) => return Err(NativeThreadStatusReadError::Unavailable),
        }
    };

    let response = match response {
        Ok(response) => response,
        Err(NativeConnectionError::Rejected { code }) => {
            let native = connection.take_last_rejection();
            if code == -32600
                && native
                    .as_ref()
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    == Some(format!("thread not loaded: {thread_id}").as_str())
            {
                return Err(NativeThreadStatusReadError::Missing);
            }
            return Err(NativeThreadStatusReadError::Rejected { code, native });
        }
        Err(
            NativeConnectionError::InvalidInput
            | NativeConnectionError::Unavailable
            | NativeConnectionError::UnavailableWithCause(_),
        ) => {
            return Err(NativeThreadStatusReadError::Unsupported);
        }
        Err(_) => return Err(NativeThreadStatusReadError::Unavailable),
    };

    if response.pointer("/thread/id").and_then(Value::as_str) != Some(thread_id) {
        return Err(NativeThreadStatusReadError::InvalidResponse);
    }
    let Some(thread) = response.get("thread").cloned() else {
        return Err(NativeThreadStatusReadError::InvalidResponse);
    };
    let status = match thread.pointer("/status/type").and_then(Value::as_str) {
        Some("notLoaded") => NativeThreadStatus::NotLoaded,
        Some("idle") => NativeThreadStatus::Idle,
        Some("active") => NativeThreadStatus::Active,
        Some(_) => NativeThreadStatus::Other,
        None => return Err(NativeThreadStatusReadError::InvalidResponse),
    };
    Ok(NativeThreadSnapshot { status, thread })
}

pub(crate) struct NativeMessageRequest<'a> {
    pub params: NativeSendParams,
    pub id: Value,
    pub service_id: &'a UuidIdentity,
    pub backend: &'a NativeControlBackend,
    pub endpoints: &'a [EndpointDescription],
    pub header_context: MessageHeaderContext,
    pub display_names: &'a crate::SessionDisplayNameCache,
    pub held_connection: Option<&'a mut NativeProtocolConnection>,
    pub load_policy: LoadPolicy,
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
    let load_policy = request.load_policy;
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
    let Ok(rendered) = collaboration_protocol::render_message_with_context(
        &params.target,
        &params.message,
        &request.header_context,
    ) else {
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
    let delivery_context = NativeMessageDeliveryContext {
        thread_id: &target_id,
        params: &params,
        header_context: &request.header_context,
        display_names: request.display_names,
        correlation: &correlation,
        held_unmaterialized: held,
        load_policy,
    };
    let result = session.deliver(&delivery_context).await;
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

struct NativeMessageDeliveryContext<'a> {
    thread_id: &'a str,
    params: &'a NativeSendParams,
    header_context: &'a MessageHeaderContext,
    display_names: &'a crate::SessionDisplayNameCache,
    correlation: &'a str,
    held_unmaterialized: bool,
    load_policy: LoadPolicy,
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
                        return Err(self.effects.native_rejection(stage, code, native.as_ref()));
                    }
                    NativeConnectionError::InvalidInput
                    | NativeConnectionError::Unavailable
                    | NativeConnectionError::UnavailableWithCause(_) => {
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
    async fn read_thread_status(&mut self, id: &str) -> Result<NativeThreadSnapshot, Value> {
        match read_native_thread_status(
            self.connection,
            self.schemas.as_ref(),
            id,
            self.deadline,
            &self.retired,
        )
        .await
        {
            Ok(snapshot) => Ok(snapshot),
            Err(NativeThreadStatusReadError::Missing) => Err(self
                .effects
                .failure("threadMissingOrUnmaterialized", "inspect")),
            Err(NativeThreadStatusReadError::Rejected { code, native }) => Err(self
                .effects
                .native_rejection("inspect", code, native.as_ref())),
            Err(NativeThreadStatusReadError::Unsupported) => {
                Err(self.effects.failure("unsupportedCapability", "inspect"))
            }
            Err(NativeThreadStatusReadError::Unavailable) => {
                Err(self.effects.failure("unavailable", "inspect"))
            }
            Err(NativeThreadStatusReadError::InvalidResponse) => {
                Err(self.effects.failure("nativeRejected", "inspect"))
            }
        }
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
        request: &NativeMessageDeliveryContext<'_>,
    ) -> Result<NativeSendAcceptance, Value> {
        let thread_snapshot = if request.held_unmaterialized {
            None
        } else {
            Some(self.read_thread_status(request.thread_id).await?)
        };
        if let Some(snapshot) = thread_snapshot.as_ref() {
            cache_thread_display_name(
                request.display_names,
                &request.params.target,
                &snapshot.thread,
            );
        }
        let current_header_context = MessageHeaderContext::resolve(
            &request.params.target,
            &request.params.message,
            request.display_names,
            request.header_context.origin,
        );
        let rendered = collaboration_protocol::render_message_with_context(
            &request.params.target,
            &request.params.message,
            &current_header_context,
        )
        .map_err(|_| self.effects.failure("overloaded", "inspect"))?;
        let input = json!([{"type":"text","text":rendered.text}]);
        let status = thread_snapshot
            .as_ref()
            .map_or(NativeThreadStatus::Idle, |snapshot| snapshot.status);
        if status == NativeThreadStatus::NotLoaded && request.load_policy == LoadPolicy::LoadedOnly
        {
            return Err(self.effects.failure(NOT_LOADED_REASON, "start"));
        }
        if request.params.delivery == MessageDelivery::Queue {
            if status == NativeThreadStatus::NotLoaded {
                return Err(self.effects.failure("threadNotLoaded", "queue"));
            }
            let result = self
                .call(
                    NativeOperation::QueueAdd,
                    json!({"threadId":request.thread_id,"input":input,"clientUserMessageId":request.correlation}),
                    "queue",
                )
                .await?;
            let submission_id = self.receipt_id(&result, "/queuedSubmission/id", "queue")?;
            return Ok(NativeSendAcceptance::QueueAccepted { submission_id });
        }
        if status == NativeThreadStatus::Active {
            let turn = self.read_active_turn_id(request.thread_id).await?;
            let result = self.call(NativeOperation::SteerTurn, json!({"threadId":request.thread_id,"expectedTurnId":turn,"input":input,"clientUserMessageId":request.correlation}), "steer").await?;
            let turn_id = self.receipt_id(&result, "/turnId", "steer")?;
            if String::from(turn_id.clone()) != turn {
                return Err(self.effects.failure("outcomeUnknown", "steer"));
            }
            return Ok(NativeSendAcceptance::SteerAccepted {
                turn_id,
                submission_id: None,
            });
        }
        if request.params.delivery == MessageDelivery::Steer {
            return Err(self.effects.failure("noActiveTurn", "steer"));
        }
        if status == NativeThreadStatus::NotLoaded {
            let resumed = self
                .call(
                    NativeOperation::ResumeThread,
                    json!({"threadId":request.thread_id,"excludeTurns":true}),
                    "resume",
                )
                .await?;
            if resumed.pointer("/thread/id").and_then(Value::as_str) != Some(request.thread_id) {
                return Err(self.effects.failure("outcomeUnknown", "resume"));
            }
            self.effects.resume = "accepted";
        }
        let result = self
            .call(
                NativeOperation::StartTurn,
                json!({"threadId":request.thread_id,"input":input,"clientUserMessageId":request.correlation}),
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

fn cache_thread_display_name(
    display_names: &crate::SessionDisplayNameCache,
    target: &SessionRef,
    thread: &Value,
) {
    let name = thread
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    match name {
        Some(name) => display_names.remember(target.clone(), name),
        None => display_names.forget(target.clone()),
    }
}
