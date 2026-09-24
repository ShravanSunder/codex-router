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
}
