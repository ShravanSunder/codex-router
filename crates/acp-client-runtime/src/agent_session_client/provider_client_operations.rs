//! Public provider Session operations after connection admission.

use super::*;

impl<P: InteractionPort> AgentSessionClient<P> {
    #[must_use]
    pub fn admission(&self) -> &ExternalProviderAdmission {
        &self.admission
    }

    pub async fn capability_report(&self, provider_session_id: &str) -> ProviderCapabilityReport {
        self.session_capabilities
            .read()
            .await
            .get(provider_session_id)
            .cloned()
            .unwrap_or_else(|| self.base_capabilities.clone())
    }

    #[must_use]
    #[cfg(any(test, feature = "test-observation"))]
    pub fn permission_observation(&self) -> ExternalProviderPermissionObservation {
        let request_count = self.permission_request_count.load(Ordering::Relaxed);
        ExternalProviderPermissionObservation {
            method: "session/request_permission",
            request_count,
            last_outcome: match self.permission_outcome.load(Ordering::Relaxed) {
                1 => Some(ExternalProviderPermissionOutcome::Cancelled),
                2 => Some(ExternalProviderPermissionOutcome::Selected),
                _ => None,
            },
            execute_tool_call_count: 0,
        }
    }

    #[cfg(any(test, feature = "test-observation"))]
    pub fn take_test_tool_calls(&self) -> Vec<ExternalProviderToolCall> {
        std::mem::take(
            &mut *self
                .test_tool_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    #[must_use]
    pub fn retirement(&self) -> CancellationToken {
        self.retirement.clone()
    }

    pub async fn set_endpoint_id(&self, endpoint_id: String) {
        *self.endpoint_id.write().await = Some(endpoint_id);
    }

    #[cfg(any(test, feature = "test-observation"))]
    pub fn approval_refusal_warnings(&self) -> Vec<ExternalProviderApprovalRefusalWarning> {
        self.approval_refusal_warnings
            .lock()
            .map(|warnings| warnings.clone())
            .unwrap_or_default()
    }

    #[cfg(any(test, feature = "test-observation"))]
    pub fn active_approval_operation(&self, provider_session_id: &str) -> Option<P::OperationId> {
        self.approval_contexts.lock().ok().and_then(|contexts| {
            contexts
                .get(provider_session_id)
                .map(|context| P::operation_id(&context.approval))
        })
    }

    #[cfg(any(test, feature = "test-observation"))]
    pub fn active_approval_count(&self) -> usize {
        self.approval_contexts
            .lock()
            .map_or(0, |contexts| contexts.len())
    }

    #[cfg(any(test, feature = "test-observation"))]
    pub async fn abort_owner_for_test(&self) {
        if let Some(task) = self.task.lock().await.as_ref() {
            task.abort();
        }
    }

    pub async fn prompt_with_approval_context(
        &self,
        provider_session_id: String,
        prompt: String,
        context: P::Context,
    ) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
        self.prompt_with_approval_dispatch(provider_session_id, prompt, context, None)
            .await
    }

    pub async fn create_session(
        &self,
        cwd: PathBuf,
    ) -> Result<String, ExternalProviderRuntimeError> {
        self.create_session_with_observation(cwd)
            .await
            .map(|created| created.provider_session_id)
    }

    pub async fn create_session_with_observation(
        &self,
        cwd: PathBuf,
    ) -> Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Create { cwd, reply })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
    }

    pub async fn load_session(
        &self,
        provider_session_id: String,
        cwd: PathBuf,
    ) -> Result<(), ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Load {
                provider_session_id,
                cwd,
                reply,
            })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
    }

    pub async fn steer_session(
        &self,
        provider_session_id: String,
        prompt: String,
    ) -> Result<ProviderSteeringOutcome<P::OperationId>, ExternalProviderRuntimeError> {
        if !self.admission.supports_steering {
            return Err(ExternalProviderRuntimeError::UnsupportedSteering);
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Steer {
                provider_session_id,
                prompt,
                reply,
            })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
    }

    pub async fn session_activity(
        &self,
        provider_session_id: String,
    ) -> Result<ProviderSessionActivity, ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::InspectSession {
                provider_session_id,
                reply,
            })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)
    }

    pub async fn wait_session_idle(
        &self,
        provider_session_id: String,
    ) -> Result<(), ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::WaitSessionIdle {
                provider_session_id,
                reply,
            })
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?;
        result
            .await
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
    }

    pub async fn shutdown(&self) {
        self.retirement.cancel();
        self.shutdown.cancel();
        if let Some(task) = self.task.lock().await.take()
            && task.await.is_err()
        {
            self.shutdown_failed.store(true, Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn shutdown_failed(&self) -> bool {
        self.shutdown_failed.load(Ordering::Relaxed)
    }
}
