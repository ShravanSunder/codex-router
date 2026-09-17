//! Retained native prompt connection with explicit terminal and callback handoff.
use crate::{
    AcpSchemaCatalog, AcpSessionBinding, NativeInterruptionState, NativePromptTerminal,
    PromptSettlement, PromptTarget, project_assistant_text, translate_prompt_content,
};
use codex_native_integration::{NativeConnectionError, NativeOperation};
use serde_json::{Value, json};

pub enum PromptEvent {
    Update(Value),
    Terminal(Value),
    /// The connection dispatcher must translate and correlate this callback; never auto-approve.
    NativeCallback(Value),
    /// Other native items remain available to the tool/plan projection owner.
    NativeNotification(Value),
}
#[derive(Debug, thiserror::Error)]
pub enum PromptExecutionError {
    #[error("invalid ACP prompt")]
    InvalidPrompt,
    #[error("native prompt request failed")]
    Native(#[from] NativeConnectionError),
    #[error("native prompt projection failed")]
    Projection,
    #[error("native prompt receipt projection failed at {0}")]
    ReceiptProjection(&'static str),
    #[error("requested effort {requested} but native runtime reported {effective}")]
    EffortMismatch {
        requested: String,
        effective: String,
    },
}
/// Seconds since the thread's own recency, taken from the read Router already made.
///
/// The native Thread schema requires `updatedAt` as unix seconds. A response
/// without it is only idle-free when the thread was never updated, so zero is a
/// fact there; anything else must fail rather than invent a recency.
fn thread_idle_seconds(thread: &Value) -> Result<u64, PromptExecutionError> {
    match thread.get("updatedAt").and_then(Value::as_i64) {
        Some(updated_at) => Ok(u64::try_from(
            chrono::Utc::now().timestamp().saturating_sub(updated_at),
        )
        .unwrap_or(0)),
        None if thread.get("createdAt").and_then(Value::as_i64).is_some() => Ok(0),
        None => Err(PromptExecutionError::ReceiptProjection("recency")),
    }
}

pub struct PendingAcpPrompt {
    next_permission: u64,
    detached: bool,
    session: AcpSessionBinding,
    settlement: PromptSettlement,
    /// Absent on resume, where the thread's persisted effort governs.
    requested_effort: Option<String>,
}
impl PendingAcpPrompt {
    /// Consuming the binding prevents a second concurrent prompt on this mapping.
    pub async fn start(
        session: AcpSessionBinding,
        catalog: &mut AcpSchemaCatalog,
        request_id: Value,
        params: &Value,
    ) -> Result<Self, PromptExecutionError> {
        if !session.schemas.supports_server_messages() {
            return Err(PromptExecutionError::Projection);
        }
        let translated = translate_prompt_content(catalog, params)
            .map_err(|_| PromptExecutionError::InvalidPrompt)?;
        let effort = match params.pointer("/_meta/codexRouter/effort") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|value| {
                        !value.trim().is_empty() && !value.chars().any(char::is_whitespace)
                    })
                    .ok_or(PromptExecutionError::InvalidPrompt)?
                    .to_owned(),
            ),
        };
        if translated.session_id() != session.session_id {
            return Err(PromptExecutionError::InvalidPrompt);
        }
        let mut pending = Self {
            next_permission: 0,
            detached: false,
            session,
            settlement: PromptSettlement::new(request_id)
                .map_err(|_| PromptExecutionError::InvalidPrompt)?,
            requested_effort: effort,
        };
        pending
            .settlement
            .mark_dispatched()
            .map_err(|_| PromptExecutionError::InvalidPrompt)?;
        // Without a requested effort the turn inherits the thread's own.
        let mut turn = json!({"threadId":pending.session.session_id,"input":translated.input()});
        if let (Some(fields), Some(effort)) =
            (turn.as_object_mut(), pending.requested_effort.as_ref())
        {
            fields.insert("effort".into(), json!(effort));
        }
        let result = pending
            .session
            .connection
            .request_validated(&pending.session.schemas, NativeOperation::StartTurn, turn)
            .await?;
        let turn_id = result
            .get("turn")
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
            .ok_or(PromptExecutionError::Projection)?;
        pending
            .settlement
            .accepted_turn(turn_id.to_owned())
            .map_err(|_| PromptExecutionError::Projection)?;
        Ok(pending)
    }
    pub async fn next_event(
        &mut self,
        catalog: &mut AcpSchemaCatalog,
    ) -> Result<Option<PromptEvent>, PromptExecutionError> {
        let message = match self.session.connection.next_message().await {
            Ok(message) => message,
            Err(_) => {
                self.detached = true;
                return Ok(self
                    .settlement
                    .observe_terminal(None, NativePromptTerminal::GenerationLost)
                    .map(PromptEvent::Terminal));
            }
        };
        if !self.session.schemas.validates_server_message(&message) {
            self.detached = true;
            return Err(PromptExecutionError::ReceiptProjection(
                "serverMessageSchema",
            ));
        }
        if message.get("id").is_some() && message.get("method").is_some() {
            if matches!(
                message.get("method").and_then(Value::as_str),
                Some("item/commandExecution/requestApproval" | "item/fileChange/requestApproval")
            ) {
                let params = message
                    .get("params")
                    .ok_or(PromptExecutionError::Projection)?;
                if params.get("threadId").and_then(Value::as_str) != Some(&self.session.session_id)
                    || params.get("turnId").and_then(Value::as_str) != self.settlement.turn_id()
                {
                    return Ok(None);
                }
                let id = crate::permission_address::permission_id(
                    &self.session.session_id,
                    self.settlement
                        .turn_id()
                        .ok_or(PromptExecutionError::Projection)?,
                    self.next_permission,
                )
                .map_err(|_| PromptExecutionError::Projection)?;
                self.next_permission = self
                    .next_permission
                    .checked_add(1)
                    .ok_or(PromptExecutionError::Projection)?;
                let permission = crate::PendingPermission::translate(
                    catalog,
                    self.session.generation.clone(),
                    id.clone(),
                    &message,
                )
                .map_err(|_| PromptExecutionError::Projection)?;
                let request = permission.request().clone();
                let outcome = self
                    .session
                    .approval_broker
                    .request(crate::BrokeredApprovalRequest {
                        thread_id: self.session.session_id.clone(),
                        generation: self.session.generation.clone(),
                        request,
                    })
                    .await
                    .map_err(|_| PromptExecutionError::Projection)?;
                let outcome = match outcome {
                    crate::BrokeredApprovalOutcome::Selected { option_id } => {
                        json!({"outcome":"selected","optionId":option_id})
                    }
                    crate::BrokeredApprovalOutcome::Cancelled => json!({"outcome":"cancelled"}),
                };
                let response = json!({"jsonrpc":"2.0","id":id,"result":{"outcome":outcome}});
                let reply = permission
                    .resolve(catalog, &self.session.generation, &response)
                    .map_err(|_| PromptExecutionError::Projection)?;
                let native_id = reply
                    .native_response
                    .get("id")
                    .ok_or(PromptExecutionError::Projection)?
                    .clone();
                let result = reply
                    .native_response
                    .get("result")
                    .ok_or(PromptExecutionError::Projection)?
                    .clone();
                self.session
                    .connection
                    .submit_callback_response(native_id, result)
                    .await?;
                return Ok(Some(PromptEvent::NativeNotification(json!({
                    "kind":"approvalDecisionSubmitted"
                }))));
            }
            return Ok(Some(PromptEvent::NativeCallback(message)));
        }
        if !self.settlement.is_settled()
            && let Some(turn_id) = self.settlement.turn_id()
            && let Some(update) = project_assistant_text(
                catalog,
                PromptTarget {
                    session_id: &self.session.session_id,
                    turn_id,
                },
                &message,
            )
            .map_err(|_| PromptExecutionError::ReceiptProjection("assistantText"))?
        {
            return Ok(Some(PromptEvent::Update(update)));
        }
        if !self.settlement.is_settled()
            && let Some(turn_id) = self.settlement.turn_id()
            && let Some(update) = crate::project_tool_progress(
                catalog,
                PromptTarget {
                    session_id: &self.session.session_id,
                    turn_id,
                },
                &message,
            )
            .map_err(|_| PromptExecutionError::ReceiptProjection("toolProgress"))?
        {
            return Ok(Some(PromptEvent::Update(update)));
        }
        if message.get("method").and_then(Value::as_str) == Some("turn/completed") {
            let params = message
                .get("params")
                .ok_or(PromptExecutionError::ReceiptProjection("terminalParams"))?;
            if params.get("threadId").and_then(Value::as_str) != Some(&self.session.session_id) {
                return Ok(None);
            }
            let turn = params
                .get("turn")
                .ok_or(PromptExecutionError::ReceiptProjection("terminalTurn"))?;
            let status = match turn.get("status").and_then(Value::as_str) {
                Some("completed") => NativePromptTerminal::Completed,
                Some("interrupted") => NativePromptTerminal::Interrupted,
                Some("failed") => NativePromptTerminal::Failed,
                _ => return Err(PromptExecutionError::ReceiptProjection("terminalStatus")),
            };
            let mut terminal = self
                .settlement
                .observe_terminal(turn.get("id").and_then(Value::as_str), status);
            if let Some(response) = terminal.as_mut() {
                let read = self
                    .session
                    .connection
                    .request_validated(
                        &self.session.schemas,
                        NativeOperation::ReadThread,
                        json!({"threadId":self.session.session_id,"includeTurns":false}),
                    )
                    .await?;
                let thread = read
                    .get("thread")
                    .ok_or(PromptExecutionError::ReceiptProjection("thread"))?;
                let model = thread
                    .get("model")
                    .and_then(Value::as_str)
                    .ok_or(PromptExecutionError::ReceiptProjection("model"))?;
                let effective_effort = thread
                    .get("reasoningEffort")
                    .and_then(Value::as_str)
                    .ok_or(PromptExecutionError::ReceiptProjection("effort"))?;
                // A resume without a requested effort reports what the thread kept.
                // A resume that asks for a different one is allowed, and says so:
                // the provider's prompt cache for this session will not be reused.
                let effort_change = match (&self.requested_effort, &self.session.persisted_effort) {
                    (Some(requested), Some(persisted)) if requested != persisted => {
                        Some(json!({"previous":persisted,"requested":requested}))
                    }
                    _ => None,
                };
                if effort_change.is_none()
                    && let Some(requested) = &self.requested_effort
                    && effective_effort != requested
                {
                    return Err(PromptExecutionError::EffortMismatch {
                        requested: requested.clone(),
                        effective: effective_effort.to_owned(),
                    });
                }
                let idle_seconds = thread_idle_seconds(thread)?;
                let result = response
                    .get_mut("result")
                    .and_then(Value::as_object_mut)
                    .ok_or(PromptExecutionError::ReceiptProjection("result"))?;
                if result.get("_meta").is_some_and(Value::is_null) {
                    result.insert("_meta".into(), json!({}));
                }
                let metadata = result
                    .entry("_meta")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or(PromptExecutionError::ReceiptProjection("metadata"))?;
                let mut router_metadata = json!({
                    "effectiveModel": model,
                    "effectiveEffort": effective_effort,
                    "effectiveAccess": self.session.requested_access,
                    "settingsObservation": &self.session.settings_observation,
                    "idleSeconds": idle_seconds
                });
                if let (Some(fields), Some(effort_change)) =
                    (router_metadata.as_object_mut(), effort_change)
                {
                    fields.insert("effortChange".into(), effort_change);
                }
                metadata.insert("codexRouter".into(), router_metadata);
            }
            return Ok(terminal.map(PromptEvent::Terminal));
        }
        Ok(Some(PromptEvent::NativeNotification(message)))
    }
    pub async fn cancel(&mut self) -> Option<Value> {
        if self.settlement.is_settled() {
            return None;
        }
        self.settlement.request_cancel();
        match tokio::time::timeout(std::time::Duration::from_secs(30), self.cancel_inner()).await {
            Ok(response) => response,
            Err(_) => {
                self.detached = true;
                self.settlement
                    .settle_cancel(NativeInterruptionState::Unknown)
            }
        }
    }
    async fn cancel_inner(&mut self) -> Option<Value> {
        let target = self.settlement.request_cancel().map(str::to_owned);
        let outcome = if let Some(turn_id) = target {
            match self
                .session
                .connection
                .request_validated(
                    &self.session.schemas,
                    NativeOperation::InterruptTurn,
                    json!({"threadId":self.session.session_id,"turnId":turn_id}),
                )
                .await
            {
                Ok(_) => NativeInterruptionState::Confirmed,
                Err(NativeConnectionError::Rejected { .. }) => NativeInterruptionState::Rejected,
                Err(_) => NativeInterruptionState::Unknown,
            }
        } else {
            NativeInterruptionState::Unknown
        };
        self.settlement.settle_cancel(outcome)
    }
    #[must_use]
    pub fn blocks_next_prompt(&self) -> bool {
        self.detached || self.settlement.blocks_next_prompt()
    }
    pub(crate) fn cancellation_barrier(&self) -> crate::CancellationBarrier {
        self.settlement
            .turn_id()
            .map_or(crate::CancellationBarrier::UnknownTurn, |turn| {
                crate::CancellationBarrier::KnownTurn(turn.to_owned())
            })
    }
    pub fn into_session(self) -> Result<AcpSessionBinding, Box<Self>> {
        if self.blocks_next_prompt() {
            Err(Box::new(self))
        } else {
            Ok(self.session)
        }
    }
}
