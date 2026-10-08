//! A provider backend that answers the Host's provider operations from a script, so the CLI
//! runs against the real collaboration API and its in-Host composition while the provider
//! itself is a fixture.
use collaboration_protocol::{
    ConversationCancelRequest, ConversationCloseRequest, ConversationCreateRequest,
    ConversationLoadRequest, ConversationOperationFailure, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, ConversationOperationSnapshot,
    ConversationOperationSubmission, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationPromptRequest, ConversationResumeRequest,
    EndpointRef, ProviderBindingIdentity, ProviderSessionInspectRequest,
    ProviderSettingsAcceptRequest, ProviderSettingsSetRequest,
};
use collaboration_service::{
    ProviderConversationBackend, ProviderConversationFuture, ProviderSessionInspectFuture,
    ProviderSettingsFuture,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

/// What the provider answers for one scripted operation.
pub enum ScriptedAnswer {
    /// The operation's typed result, as JSON.
    Answer(Value),
    /// A `ConversationOperationFailure`, as JSON.
    Failure(Value),
    /// The provider never answers.
    Never,
}

/// One expected provider operation: its backend method and how it answers the request the
/// Host sent.
pub struct ScriptedStep {
    method: &'static str,
    respond: Box<dyn FnOnce(&Value) -> ScriptedAnswer + Send>,
}

impl ScriptedStep {
    /// Checks the request, then gives `answer`.
    pub fn new(
        method: &'static str,
        check: impl FnOnce(&Value) + Send + 'static,
        answer: ScriptedAnswer,
    ) -> Self {
        Self::responding(method, move |request| {
            check(request);
            answer
        })
    }

    /// Answers from the request itself.
    pub fn responding(
        method: &'static str,
        respond: impl FnOnce(&Value) -> ScriptedAnswer + Send + 'static,
    ) -> Self {
        Self {
            method,
            respond: Box::new(respond),
        }
    }
}

#[derive(Clone)]
pub struct ScriptedProviderBackend {
    binding: ProviderBindingIdentity,
    steps: Arc<Mutex<VecDeque<ScriptedStep>>>,
}

impl ScriptedProviderBackend {
    pub fn new(binding: ProviderBindingIdentity, steps: Vec<ScriptedStep>) -> Self {
        Self {
            binding,
            steps: Arc::new(Mutex::new(steps.into())),
        }
    }

    /// The scripted operations the Host has not performed.
    pub fn remaining(&self) -> Vec<&'static str> {
        self.steps
            .lock()
            .map(|steps| steps.iter().map(|step| step.method).collect())
            .unwrap_or_default()
    }

    fn next_step(&self, method: &'static str, request: &Value) -> ScriptedAnswer {
        let step = self
            .steps
            .lock()
            .ok()
            .and_then(|mut steps| steps.pop_front());
        let Some(step) = step else {
            panic!("unscripted provider operation {method}: {request}");
        };
        assert_eq!(step.method, method, "provider operation order: {request}");
        (step.respond)(request)
    }

    fn answer<TRequest: Serialize, TResult: DeserializeOwned + Send + 'static>(
        &self,
        method: &'static str,
        request: TRequest,
    ) -> ProviderConversationFuture<'_, TResult> {
        let request = serde_json::to_value(request).expect("provider request JSON");
        let answer = self.next_step(method, &request);
        Box::pin(async move {
            match answer {
                ScriptedAnswer::Answer(value) => {
                    Ok(serde_json::from_value(value).expect("scripted provider result"))
                }
                ScriptedAnswer::Failure(value) => Err(serde_json::from_value::<
                    ConversationOperationFailure,
                >(value)
                .expect("scripted provider failure")),
                ScriptedAnswer::Never => std::future::pending().await,
            }
        })
    }

    fn settled<TRequest: Serialize, TResult: DeserializeOwned>(
        &self,
        method: &'static str,
        request: TRequest,
    ) -> TResult {
        let request = serde_json::to_value(request).expect("provider request JSON");
        match self.next_step(method, &request) {
            ScriptedAnswer::Answer(value) => {
                serde_json::from_value(value).expect("scripted provider result")
            }
            ScriptedAnswer::Failure(_) | ScriptedAnswer::Never => {
                panic!("{method} is scripted with answers only")
            }
        }
    }
}

impl ProviderConversationBackend for ScriptedProviderBackend {
    fn binding(&self, endpoint: &EndpointRef) -> Option<ProviderBindingIdentity> {
        (&self.binding.endpoint == endpoint).then(|| self.binding.clone())
    }

    fn inspect_session(
        &self,
        request: ProviderSessionInspectRequest,
    ) -> ProviderSessionInspectFuture<'_> {
        let result = self.settled("inspectSession", request);
        Box::pin(async move { Ok(result) })
    }

    fn settings_set(&self, request: ProviderSettingsSetRequest) -> ProviderSettingsFuture<'_> {
        let result = self.settled("settingsSet", request);
        Box::pin(async move { Ok(result) })
    }

    fn settings_accept(
        &self,
        request: ProviderSettingsAcceptRequest,
    ) -> ProviderSettingsFuture<'_> {
        let result = self.settled("settingsAccept", request);
        Box::pin(async move { Ok(result) })
    }

    fn create(
        &self,
        request: ConversationCreateRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.answer("create", request)
    }

    fn load(
        &self,
        request: ConversationLoadRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.answer("load", request)
    }

    fn resume(
        &self,
        request: ConversationResumeRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.answer("resume", request)
    }

    fn close(
        &self,
        request: ConversationCloseRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.answer("close", request)
    }

    fn prompt(
        &self,
        request: ConversationPromptRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.answer("prompt", request)
    }

    fn cancel(
        &self,
        request: ConversationCancelRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        self.answer("cancel", request)
    }

    fn show(
        &self,
        request: ConversationOperationShowRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
        self.answer("show", request)
    }

    fn wait(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationWaitResult> {
        self.answer("wait", request)
    }

    fn reconcile(
        &self,
        request: ConversationOperationReconcileRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
        self.answer("reconcile", request)
    }
}
