//! Retained native prompt connection with explicit terminal and callback handoff.
use crate::{
    AcpSchemaCatalog, AcpSessionBinding, NativeInterruptionState, NativePromptTerminal,
    PromptSettlement, PromptTarget, project_assistant_text, translate_prompt_content,
};
use codex_native_integration::{NativeConnectionError, NativeOperation};
use serde_json::{Value, json};

pub enum PromptEvent {
    PermissionRequest(Value),
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
}
pub struct PendingAcpPrompt {
    permissions: std::collections::BTreeMap<String, crate::PendingPermission>,
    next_permission: u64,
    detached: bool,
    session: AcpSessionBinding,
    settlement: PromptSettlement,
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
        if translated.session_id() != session.session_id {
            return Err(PromptExecutionError::InvalidPrompt);
        }
        let mut pending = Self {
            permissions: std::collections::BTreeMap::new(),
            next_permission: 0,
            detached: false,
            session,
            settlement: PromptSettlement::new(request_id)
                .map_err(|_| PromptExecutionError::InvalidPrompt)?,
        };
        pending
            .settlement
            .mark_dispatched()
            .map_err(|_| PromptExecutionError::InvalidPrompt)?;
        let result = pending
            .session
            .connection
            .request_validated(
                &pending.session.schemas,
                NativeOperation::StartTurn,
                json!({"threadId":pending.session.session_id,"input":translated.input()}),
            )
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
            return Err(PromptExecutionError::Projection);
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
                if self.permissions.len() >= 64 {
                    return Err(PromptExecutionError::Projection);
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
                self.permissions.insert(id, permission);
                return Ok(Some(PromptEvent::PermissionRequest(request)));
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
            .map_err(|_| PromptExecutionError::Projection)?
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
            .map_err(|_| PromptExecutionError::Projection)?
        {
            return Ok(Some(PromptEvent::Update(update)));
        }
        if message.get("method").and_then(Value::as_str) == Some("turn/completed") {
            let params = message
                .get("params")
                .ok_or(PromptExecutionError::Projection)?;
            if params.get("threadId").and_then(Value::as_str) != Some(&self.session.session_id) {
                return Ok(None);
            }
            let turn = params.get("turn").ok_or(PromptExecutionError::Projection)?;
            let status = match turn.get("status").and_then(Value::as_str) {
                Some("completed") => NativePromptTerminal::Completed,
                Some("interrupted") => NativePromptTerminal::Interrupted,
                Some("failed") => NativePromptTerminal::Failed,
                _ => return Err(PromptExecutionError::Projection),
            };
            return Ok(self
                .settlement
                .observe_terminal(turn.get("id").and_then(Value::as_str), status)
                .map(PromptEvent::Terminal));
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
                self.permissions.clear();
                self.settlement
                    .settle_cancel(NativeInterruptionState::Unknown)
            }
        }
    }
    async fn cancel_inner(&mut self) -> Option<Value> {
        for (_, permission) in std::mem::take(&mut self.permissions) {
            let response = permission.cancel();
            if let (Some(id), Some(result)) = (response.get("id"), response.get("result")) {
                let _submitted = self
                    .session
                    .connection
                    .submit_callback_response(id.clone(), result.clone())
                    .await;
            }
        }
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
    pub async fn respond_permission(
        &mut self,
        catalog: &mut AcpSchemaCatalog,
        response: &Value,
    ) -> Result<Option<Value>, PromptExecutionError> {
        let id = response
            .get("id")
            .and_then(Value::as_str)
            .ok_or(PromptExecutionError::Projection)?;
        let Some(permission) = self.permissions.remove(id) else {
            return Ok(None);
        };
        let reply = permission
            .resolve(catalog, &self.session.generation, response)
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
        if reply.invalid_selection {
            self.detached = true;
            let turn = self.settlement.turn_id().map(str::to_owned);
            let mut terminal = self
                .settlement
                .observe_terminal(turn.as_deref(), NativePromptTerminal::Failed);
            if let Some(response) = &mut terminal
                && let Some(error) = response.get_mut("error").and_then(Value::as_object_mut)
            {
                error.insert("message".into(), json!("Invalid permission option"));
            }
            return Ok(terminal);
        }
        Ok(None)
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
