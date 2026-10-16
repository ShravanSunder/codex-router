//! The invocation-local record reload channel; no catalog or source policy.
use super::{
    picker_request::SessionsPickerDataQuery, source_reload_worker::SessionRecordsReloadRequest,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(super) struct SessionRecordsReloadPort {
    sender: tokio::sync::watch::Sender<SessionRecordsReloadRequest>,
    receiver: Arc<Mutex<Option<tokio::sync::watch::Receiver<SessionRecordsReloadRequest>>>>,
}

impl SessionRecordsReloadPort {
    pub(super) fn new(
        initial_query: SessionsPickerDataQuery,
        sources: Vec<super::PickerSourceContext>,
    ) -> Self {
        let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
            generation: 0,
            query: initial_query,
            sources,
        });
        Self {
            sender,
            receiver: Arc::new(Mutex::new(Some(receiver))),
        }
    }

    pub(super) fn send(&self, request: SessionRecordsReloadRequest) {
        self.sender.send_replace(request);
    }

    pub(super) fn take_receiver(
        &self,
    ) -> Option<tokio::sync::watch::Receiver<SessionRecordsReloadRequest>> {
        self.receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}
