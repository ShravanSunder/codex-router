//! Per-ACP-connection session bindings and prompt actors; native state stays in Codex.
use crate::{
    AcpOutputSender, AcpSchemaCatalog, AcpSessionBinding, PromptCommand, PromptTaskInputs,
    run_prompt_task, translate_prompt_content,
};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

enum SessionSlot {
    Ready(Box<AcpSessionBinding>),
    Busy(mpsc::Sender<PromptCommand>),
    Detached,
    Loading,
}
pub enum HeldBindingCheckout {
    Ready(Box<AcpSessionBinding>),
    Busy,
    Missing,
}

/// Host-owned bindings survive an ACP frontend connection until the first turn starts.
pub trait UnmaterializedBindingStore: Send + Sync {
    fn hold(&self, binding: AcpSessionBinding);
    fn checkout(&self, session_id: &str) -> HeldBindingCheckout;
    fn restore(&self, binding: AcpSessionBinding);
    fn finish(&self, session_id: &str);
    /// Host-lifetime work that must outlive its ACP frontend connection.
    fn host_tasks(&self) -> tokio_util::task::TaskTracker;
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
// Both a failed send and a delivered-but-unobserved result retain the binding.
// Taking the completion transfers responsibility to the registry's Ready slot.
struct OwnedPromptCompletion {
    completion: crate::PromptTaskCompletion,
    holder: Arc<dyn UnmaterializedBindingStore>,
}
impl Drop for OwnedPromptCompletion {
    fn drop(&mut self) {
        if let Some(binding) = self.completion.binding.take()
            && binding.is_unmaterialized()
        {
            self.holder.hold(binding);
        }
    }
}

fn spawn_host_prompt(
    inputs: PromptTaskInputs,
    holder: Arc<dyn UnmaterializedBindingStore>,
) -> oneshot::Receiver<OwnedPromptCompletion> {
    let (completion_sender, completion_receiver) = oneshot::channel();
    holder.host_tasks().spawn(async move {
        let completion = run_prompt_task(inputs).await;
        // A dropped observer returns ownership here; the guard also covers
        // delivery followed by an observer disappearing before consumption.
        let _sent = completion_sender.send(OwnedPromptCompletion { completion, holder });
    });
    completion_receiver
}

pub struct AcpSessionRegistry {
    sessions: BTreeMap<String, SessionSlot>,
    cancellation_barriers: BTreeMap<String, crate::CancellationBarrier>,
    prompts: JoinSet<Result<(String, OwnedPromptCompletion), SessionRegistryError>>,
    output: AcpOutputSender,
    retired: CancellationToken,
    holder: Arc<dyn UnmaterializedBindingStore>,
}
impl AcpSessionRegistry {
    #[must_use]
    pub fn new(
        output: AcpOutputSender,
        retired: CancellationToken,
        holder: Arc<dyn UnmaterializedBindingStore>,
    ) -> Self {
        Self {
            sessions: BTreeMap::new(),
            cancellation_barriers: BTreeMap::new(),
            prompts: JoinSet::new(),
            output,
            retired,
            holder,
        }
    }
    pub fn insert(
        &mut self,
        session: AcpSessionBinding,
    ) -> Result<(), (SessionRegistryError, Box<AcpSessionBinding>)> {
        let id = session.session_id().to_owned();
        if matches!(self.sessions.get(&id), Some(SessionSlot::Busy(_))) {
            return Err((SessionRegistryError::Busy, Box::new(session)));
        }
        if !self.sessions.contains_key(&id) && self.sessions.len() >= 64 {
            return Err((SessionRegistryError::Capacity, Box::new(session)));
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
        let completion_receiver = spawn_host_prompt(
            PromptTaskInputs {
                session: *session,
                request_id,
                params,
                commands,
                output,
                retired,
            },
            Arc::clone(&self.holder),
        );
        self.prompts.spawn(async move {
            completion_receiver
                .await
                .map(|completion| (id, completion))
                .map_err(|_| SessionRegistryError::NotLoaded)
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
            Ok(Ok((id, mut owned))) => {
                let completed = std::mem::take(&mut owned.completion);
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
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SessionRegistryError::NotLoaded),
        }
    }
    /// Detaches this connection's observers; Host-owned native actors keep serving.
    pub async fn shutdown(self) {
        drop(self);
    }
}
impl Drop for AcpSessionRegistry {
    fn drop(&mut self) {
        // Command sender EOF detaches actors without native cancellation. Track
        // observer destruction too so Host drain includes completion recovery.
        let mut prompts = std::mem::take(&mut self.prompts);
        if !prompts.is_empty() {
            prompts.abort_all();
            self.holder
                .host_tasks()
                .spawn(async move { while prompts.join_next().await.is_some() {} });
        }
        for (_, slot) in std::mem::take(&mut self.sessions) {
            if let SessionSlot::Ready(binding) = slot
                && binding.is_unmaterialized()
            {
                self.holder.hold(*binding);
            }
        }
    }
}

#[cfg(test)]
use crate as adapter;
#[cfg(test)]
#[path = "../tests/support/actor_lifetime_fixture.rs"]
pub(crate) mod actor_lifetime_fixture;
#[cfg(test)]
#[path = "session_connection_registry_tests.rs"]
mod tests;
