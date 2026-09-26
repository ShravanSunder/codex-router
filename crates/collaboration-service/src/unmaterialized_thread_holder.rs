//! Host-lifetime ownership of Codex threads that have not started a first turn.
use codex_acp_adapter::{AcpSessionBinding, HeldBindingCheckout, UnmaterializedBindingStore};
use std::{collections::BTreeMap, sync::Mutex};

enum HeldThread {
    Ready(Box<AcpSessionBinding>),
    Busy,
}

#[derive(Default)]
pub struct UnmaterializedThreadHolder {
    threads: Mutex<BTreeMap<String, HeldThread>>,
    create_tasks: tokio_util::task::TaskTracker,
}

impl UnmaterializedThreadHolder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn contains(&self, session_id: &str) -> bool {
        self.threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(session_id)
    }

    pub async fn drain_create_tasks(&self) {
        self.create_tasks.close();
        self.create_tasks.wait().await;
    }
}

impl UnmaterializedBindingStore for UnmaterializedThreadHolder {
    fn hold(&self, binding: AcpSessionBinding) {
        if !binding.is_unmaterialized() {
            return;
        }
        self.threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                binding.session_id().to_owned(),
                HeldThread::Ready(Box::new(binding)),
            );
    }

    fn checkout(&self, session_id: &str) -> HeldBindingCheckout {
        let mut threads = self
            .threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match threads.insert(session_id.to_owned(), HeldThread::Busy) {
            Some(HeldThread::Ready(binding)) => HeldBindingCheckout::Ready(binding),
            Some(HeldThread::Busy) => HeldBindingCheckout::Busy,
            None => {
                threads.remove(session_id);
                HeldBindingCheckout::Missing
            }
        }
    }

    fn restore(&self, binding: AcpSessionBinding) {
        self.hold(binding);
    }

    fn finish(&self, session_id: &str) {
        let mut threads = self
            .threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(threads.get(session_id), Some(HeldThread::Busy)) {
            threads.remove(session_id);
        }
    }

    fn create_tasks(&self) -> tokio_util::task::TaskTracker {
        self.create_tasks.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_drains_in_flight_codex_create_task() {
        let holder = std::sync::Arc::new(UnmaterializedThreadHolder::new());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        holder.create_tasks().spawn(async move {
            let _entered = entered_tx.send(());
            let _released = release_rx.await;
        });
        entered_rx.await.expect("create task started");
        let draining = tokio::spawn({
            let holder = std::sync::Arc::clone(&holder);
            async move { holder.drain_create_tasks().await }
        });
        tokio::task::yield_now().await;
        assert!(!draining.is_finished());
        let _released = release_tx.send(());
        draining.await.expect("create tasks drained");
    }
}
