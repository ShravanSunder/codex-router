//! Per-ACP-connection session bindings and prompt actors; native state stays in Codex.
use crate::{
    AcpOutputSender, AcpSchemaCatalog, AcpSessionBinding, PromptCommand, PromptTaskInputs,
    run_prompt_task, translate_prompt_content,
};
use serde_json::Value;
use std::collections::BTreeMap;
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;

enum SessionSlot {
    Ready(Box<AcpSessionBinding>),
    Busy(mpsc::Sender<PromptCommand>),
    Detached,
    Loading,
}
#[derive(Debug, thiserror::Error)]
pub enum SessionRegistryError {
    #[error("session not loaded on this connection")]
    NotLoaded,
    #[error("session already has a pending prompt")]
    Busy,
    #[error("invalid prompt or callback parameters")]
    InvalidParameters,
    #[error("ACP session capacity exceeded")]
    Capacity,
}
pub struct AcpSessionRegistry {
    sessions: BTreeMap<String, SessionSlot>,
    cancellation_barriers: BTreeMap<String, crate::CancellationBarrier>,
    prompts: JoinSet<(String, crate::PromptTaskCompletion)>,
    output: AcpOutputSender,
    retired: CancellationToken,
}
impl AcpSessionRegistry {
    #[must_use]
    pub fn new(output: AcpOutputSender, retired: CancellationToken) -> Self {
        Self {
            sessions: BTreeMap::new(),
            cancellation_barriers: BTreeMap::new(),
            prompts: JoinSet::new(),
            output,
            retired,
        }
    }
    pub fn insert(&mut self, session: AcpSessionBinding) -> Result<(), SessionRegistryError> {
        let id = session.session_id().to_owned();
        if matches!(self.sessions.get(&id), Some(SessionSlot::Busy(_))) {
            return Err(SessionRegistryError::Busy);
        }
        if !self.sessions.contains_key(&id) && self.sessions.len() >= 64 {
            return Err(SessionRegistryError::Capacity);
        }
        self.sessions
            .insert(id, SessionSlot::Ready(Box::new(session)));
        Ok(())
    }
    pub fn begin_prompt(
        &mut self,
        catalog: &mut AcpSchemaCatalog,
        request_id: Value,
        params: Value,
    ) -> Result<(), SessionRegistryError> {
        let translated = translate_prompt_content(catalog, &params)
            .map_err(|_| SessionRegistryError::InvalidParameters)?;
        let id = translated.session_id().to_owned();
        if self.cancellation_barriers.contains_key(&id) {
            return Err(SessionRegistryError::NotLoaded);
        }
        let slot = self
            .sessions
            .get_mut(&id)
            .ok_or(SessionRegistryError::NotLoaded)?;
        match slot {
            SessionSlot::Busy(_) | SessionSlot::Loading => return Err(SessionRegistryError::Busy),
            SessionSlot::Detached => return Err(SessionRegistryError::NotLoaded),
            SessionSlot::Ready(_) => {}
        }
        let SessionSlot::Ready(session) = std::mem::replace(slot, SessionSlot::Detached) else {
            return Err(SessionRegistryError::NotLoaded);
        };
        let (sender, commands) = mpsc::channel(64);
        *slot = SessionSlot::Busy(sender);
        let output = self.output.clone();
        let retired = self.retired.clone();
        self.prompts.spawn(async move {
            let binding = run_prompt_task(PromptTaskInputs {
                session: *session,
                request_id,
                params,
                commands,
                output,
                retired,
            })
            .await;
            (id, binding)
        });
        Ok(())
    }
    pub fn cancel(&self, session_id: &str) -> Result<(), SessionRegistryError> {
        match self.sessions.get(session_id) {
            Some(SessionSlot::Busy(sender)) => sender
                .try_send(PromptCommand::Cancel)
                .map_err(|_| SessionRegistryError::Capacity),
            Some(_) => Ok(()),
            None => Err(SessionRegistryError::NotLoaded),
        }
    }
    pub fn permission_response(&self, response: Value) -> Result<(), SessionRegistryError> {
        let session = response
            .get("id")
            .and_then(Value::as_str)
            .and_then(crate::permission_address::permission_session)
            .ok_or(SessionRegistryError::InvalidParameters)?;
        if let Some(SessionSlot::Busy(sender)) = self.sessions.get(&session) {
            sender
                .try_send(PromptCommand::PermissionResponse(response))
                .map_err(|_| SessionRegistryError::Capacity)?;
        }
        Ok(())
    }
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.prompts.is_empty()
    }
    pub(crate) fn cancellation_barrier(
        &self,
        session_id: &str,
    ) -> Option<crate::CancellationBarrier> {
        self.cancellation_barriers.get(session_id).cloned()
    }
    pub(crate) fn clear_cancellation_barrier(&mut self, session_id: &str) {
        self.cancellation_barriers.remove(session_id);
    }
    pub(crate) fn reserve_load(
        &mut self,
        session_id: &str,
        pending_setups: usize,
    ) -> Result<Option<AcpSessionBinding>, SessionRegistryError> {
        if matches!(
            self.sessions.get(session_id),
            Some(SessionSlot::Busy(_) | SessionSlot::Loading)
        ) {
            return Err(SessionRegistryError::Busy);
        }
        if !self.sessions.contains_key(session_id) && !self.has_setup_capacity(pending_setups) {
            return Err(SessionRegistryError::Capacity);
        }
        let previous = self
            .sessions
            .insert(session_id.to_owned(), SessionSlot::Loading);
        Ok(match previous {
            Some(SessionSlot::Ready(session)) => Some(*session),
            _ => None,
        })
    }
    pub(crate) fn finish_failed_load(&mut self, session_id: &str) {
        if matches!(self.sessions.get(session_id), Some(SessionSlot::Loading)) {
            self.sessions
                .insert(session_id.to_owned(), SessionSlot::Detached);
        }
    }
    pub(crate) fn has_setup_capacity(&self, pending_setups: usize) -> bool {
        self.sessions.len().saturating_add(pending_setups) < 64
    }
    pub async fn complete_next(&mut self) -> Result<(), SessionRegistryError> {
        let Some(completed) = self.prompts.join_next().await else {
            return Ok(());
        };
        match completed {
            Ok((id, completed)) => {
                if let Some(barrier) = completed.cancellation_barrier {
                    self.cancellation_barriers.insert(id.clone(), barrier);
                }
                self.sessions.insert(
                    id,
                    completed.binding.map_or(SessionSlot::Detached, |session| {
                        SessionSlot::Ready(Box::new(session))
                    }),
                );
                if let Some(terminal) = completed.terminal {
                    self.output
                        .send(terminal)
                        .await
                        .map_err(|_| SessionRegistryError::Capacity)?;
                }
                Ok(())
            }
            Err(_) => {
                self.retired.cancel();
                Err(SessionRegistryError::NotLoaded)
            }
        }
    }
    /// Closes only this connection's prompt actors; the Host/backend lifecycle remains separate.
    pub async fn shutdown(mut self) {
        for slot in self.sessions.values() {
            if let SessionSlot::Busy(sender) = slot {
                let _sent = sender.try_send(PromptCommand::Cancel);
            }
        }
        let drained = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while self.prompts.join_next().await.is_some() {}
        })
        .await;
        self.retired.cancel();
        if drained.is_err() {
            self.prompts.abort_all();
            while self.prompts.join_next().await.is_some() {}
        }
    }
}
