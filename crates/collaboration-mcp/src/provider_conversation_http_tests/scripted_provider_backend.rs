//! A provider backend that answers each operation from a script and records every request,
//! standing in for the Host's provider work behind the collaboration application.
use collaboration_protocol::{
    ConversationCancelRequest, ConversationCloseRequest, ConversationCreateRequest,
    ConversationLoadRequest, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, ConversationOperationSnapshot,
    ConversationOperationSubmission, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationPromptRequest, ConversationResumeRequest,
    EndpointRef, ProviderBindingIdentity,
};
use collaboration_service::{ProviderConversationBackend, ProviderConversationFuture};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

/// One recorded provider operation: its name and the request it received, as JSON.
pub(super) type RecordedCall = (&'static str, Value);

#[derive(Clone)]
pub(super) struct ScriptedProviderBackend {
    binding: ProviderBindingIdentity,
    answers: Arc<Mutex<HashMap<&'static str, VecDeque<Value>>>>,
    calls: Arc<Mutex<Vec<RecordedCall>>>,
    /// Operations answered `{"hold": true}`: started, and dropped before they answered.
    held: Arc<(AtomicUsize, AtomicUsize)>,
}

/// Counts a held operation as dropped when the Router stops waiting for it.
struct HeldOperation(Arc<(AtomicUsize, AtomicUsize)>);

impl Drop for HeldOperation {
    fn drop(&mut self) {
        self.0.1.fetch_add(1, Ordering::SeqCst);
    }
}

impl ScriptedProviderBackend {
    pub(super) fn new() -> Self {
        Self {
            binding: serde_json::from_value(super::provider_binding()).expect("provider binding"),
            answers: Arc::default(),
            calls: Arc::default(),
            held: Arc::default(),
        }
    }

    /// Held operations that started, and those dropped without an answer.
    pub(super) fn held_operations(&self) -> (usize, usize) {
        (
            self.held.0.load(Ordering::SeqCst),
            self.held.1.load(Ordering::SeqCst),
        )
    }

    /// Queues the next answer for `operation`: `{"result": ...}`, `{"error": <failure>}`, or
    /// `{"hold": true}` for an operation that never answers.
    pub(super) fn answer(self, operation: &'static str, answer: Value) -> Self {
        self.answers
            .lock()
            .expect("script lock")
            .entry(operation)
            .or_default()
            .push_back(answer);
        self
    }

    pub(super) fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn respond<TResult: DeserializeOwned + Send + 'static>(
        &self,
        operation: &'static str,
        request: &impl Serialize,
    ) -> ProviderConversationFuture<'_, TResult> {
        self.calls.lock().expect("calls lock").push((
            operation,
            serde_json::to_value(request).expect("request JSON"),
        ));
        let answer = self
            .answers
            .lock()
            .expect("script lock")
            .get_mut(operation)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| {
                json!({"error":{
                    "kind":"protocolViolation","stage":"admission","effect":"none",
                    "message":format!("unscripted provider {operation}")
                }})
            });
        if answer.get("hold").is_some() {
            // Never answers and ignores cancellation, like a provider that stopped replying.
            self.held.0.fetch_add(1, Ordering::SeqCst);
            let held = HeldOperation(Arc::clone(&self.held));
            return Box::pin(async move {
                let _held = held;
                std::future::pending().await
            });
        }
        let answered =
            match answer.get("error") {
                Some(failure) => {
                    Err(serde_json::from_value(failure.clone()).expect("scripted provider failure"))
                }
                None => Ok(serde_json::from_value(answer["result"].clone())
                    .expect("scripted provider result")),
            };
        Box::pin(async move { answered })
    }
}

impl ProviderConversationBackend for ScriptedProviderBackend {
    fn binding(&self, endpoint: &EndpointRef) -> Option<ProviderBindingIdentity> {
        (&self.binding.endpoint == endpoint).then(|| self.binding.clone())
    }
    fn create(
        &self,
        request: ConversationCreateRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.respond("create", &request)
    }
    fn load(
        &self,
        request: ConversationLoadRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.respond("load", &request)
    }
    fn resume(
        &self,
        request: ConversationResumeRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.respond("resume", &request)
    }
    fn close(
        &self,
        request: ConversationCloseRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.respond("close", &request)
    }
    fn prompt(
        &self,
        request: ConversationPromptRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.respond("prompt", &request)
    }
    fn cancel(
        &self,
        request: ConversationCancelRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.respond("cancel", &request)
    }
    fn show(
        &self,
        request: ConversationOperationShowRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
        self.respond("show", &request)
    }
    fn wait(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationWaitResult> {
        self.respond("wait", &request)
    }
    fn reconcile(
        &self,
        request: ConversationOperationReconcileRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
        self.respond("reconcile", &request)
    }
}
