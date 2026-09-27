//! Public provider Session operations after connection admission.

use super::*;
use crate::provider_prompt_content::acp_blocks_from_prompt_content;
use agent_client_protocol::schema::v1::ContentBlock;
use session_event_model::PromptContent;

impl<P: InteractionPort> AgentSessionClient<P> {
    fn retired_operation_error(&self) -> ExternalProviderRuntimeError {
        if self.sink_closed.is_cancelled() {
            ExternalProviderRuntimeError::SinkClosed
        } else {
            ExternalProviderRuntimeError::TransportFailure
        }
    }

    #[must_use]
    pub fn admission(&self) -> &ExternalProviderAdmission {
        &self.admission
    }

    pub async fn capability_report(&self, provider_session_id: &str) -> ProviderCapabilityReport {
        let report = self
            .session_capabilities
            .read()
            .await
            .get(provider_session_id)
            .cloned()
            .unwrap_or_else(|| self.base_capabilities.clone());
        report.with_auth_status(self.auth_status.read().await.clone())
    }

    pub async fn settings_catalog(
        &self,
        provider_session_id: &str,
    ) -> Option<crate::ProviderSettingsCatalog> {
        self.session_settings
            .read()
            .await
            .get(provider_session_id)
            .cloned()
    }

    pub async fn last_settings_catalog(&self) -> Option<crate::ProviderSettingsCatalog> {
        self.last_settings_catalog.read().await.clone()
    }

    pub async fn settings_unresolved(&self, provider_session_id: &str) -> bool {
        self.settings_unresolved
            .read()
            .await
            .contains_key(provider_session_id)
    }

    pub async fn accept_session_settings(
        &self,
        provider_session_id: String,
    ) -> Result<crate::EffectiveProviderSettings, ExternalProviderRuntimeError> {
        let catalog = self
            .session_settings
            .read()
            .await
            .get(&provider_session_id)
            .cloned()
            .ok_or(ExternalProviderRuntimeError::LocalNotFound)?;
        self.settings_unresolved
            .write()
            .await
            .remove(&provider_session_id);
        Ok(catalog.effective_settings())
    }

    pub async fn set_setting(
        &self,
        provider_session_id: String,
        kind: crate::ProviderSettingKind,
        value: String,
    ) -> Result<crate::EffectiveProviderSettings, ExternalProviderRuntimeError> {
        let uncertain_session_id = provider_session_id.clone();
        let uncertain_value = value.clone();
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::SetSetting {
                provider_session_id,
                kind,
                value,
                reply,
            })
            .await
            .map_err(|_| self.retired_operation_error())?;
        match result.await {
            Ok(result) => result,
            Err(_) => {
                self.settings_unresolved
                    .write()
                    .await
                    .insert(uncertain_session_id.clone(), kind);
                Err(ExternalProviderRuntimeError::SettingOutcomeUnknown {
                    provider_session_id: uncertain_session_id,
                    setting: kind,
                    value: uncertain_value,
                })
            }
        }
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
        self.create_session_with_settings(cwd, crate::RequestedProviderSettings::default())
            .await
    }

    pub async fn create_session_with_settings(
        &self,
        cwd: PathBuf,
        settings: crate::RequestedProviderSettings,
    ) -> Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Create {
                cwd,
                settings,
                reply,
            })
            .await
            .map_err(|_| self.retired_operation_error())?;
        result.await.map_err(|_| self.retired_operation_error())?
    }

    pub async fn load_session(
        &self,
        provider_session_id: String,
        cwd: PathBuf,
    ) -> Result<(), ExternalProviderRuntimeError> {
        if !self.base_capabilities.supports_load {
            return Err(ExternalProviderRuntimeError::UnsupportedCapability {
                capability: "session/load",
            });
        }
        self.restore_session(
            provider_session_id,
            cwd,
            super::provider_session_restore::RestoreHistoryMode::Replay,
        )
        .await
    }

    pub async fn resume_session(
        &self,
        provider_session_id: String,
        cwd: PathBuf,
    ) -> Result<(), ExternalProviderRuntimeError> {
        if !self.base_capabilities.supports_resume {
            return Err(ExternalProviderRuntimeError::UnsupportedCapability {
                capability: "session/resume",
            });
        }
        self.restore_session(
            provider_session_id,
            cwd,
            super::provider_session_restore::RestoreHistoryMode::WithoutReplay,
        )
        .await
    }

    async fn restore_session(
        &self,
        provider_session_id: String,
        cwd: PathBuf,
        mode: super::provider_session_restore::RestoreHistoryMode,
    ) -> Result<(), ExternalProviderRuntimeError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Restore {
                provider_session_id,
                cwd,
                mode,
                reply,
            })
            .await
            .map_err(|_| self.retired_operation_error())?;
        result.await.map_err(|_| self.retired_operation_error())?
    }

    pub async fn list_sessions(
        &self,
        cwd: Option<PathBuf>,
    ) -> Result<Vec<super::ProviderSessionSummary>, ExternalProviderRuntimeError> {
        if !self.base_capabilities.supports_list {
            return Err(ExternalProviderRuntimeError::UnsupportedCapability {
                capability: "session/list",
            });
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::List { cwd, reply })
            .await
            .map_err(|_| self.retired_operation_error())?;
        result.await.map_err(|_| self.retired_operation_error())?
    }

    pub async fn close_session(
        &self,
        provider_session_id: String,
    ) -> Result<(), ExternalProviderRuntimeError> {
        if !self.base_capabilities.supports_close {
            return Err(ExternalProviderRuntimeError::UnsupportedCapability {
                capability: "session/close",
            });
        }
        if self.session_activity(provider_session_id.clone()).await?
            == ProviderSessionActivity::Running
        {
            match self.cancel_active_prompt(provider_session_id.clone()).await {
                Ok(()) | Err(ExternalProviderRuntimeError::LocalNotFound) => {}
                Err(error) => return Err(error),
            }
            self.wait_session_idle(provider_session_id.clone()).await?;
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Close {
                provider_session_id,
                reply,
            })
            .await
            .map_err(|_| self.retired_operation_error())?;
        result.await.map_err(|_| self.retired_operation_error())?
    }

    /// Steer with the same validated Session vocabulary used for prompts.
    /// Optional ACP content types are checked before any command is queued.
    pub async fn steer_contents_with_input(
        &self,
        provider_session_id: String,
        input_id: InputId,
        contents: Vec<PromptContent>,
    ) -> Result<ProviderSteeringOutcome<P::OperationId>, ExternalProviderRuntimeError> {
        self.steer_acp_blocks_with_input(
            provider_session_id,
            input_id,
            acp_blocks_from_prompt_content(contents),
        )
        .await
    }

    async fn steer_acp_blocks_with_input(
        &self,
        provider_session_id: String,
        input_id: InputId,
        blocks: Vec<ContentBlock>,
    ) -> Result<ProviderSteeringOutcome<P::OperationId>, ExternalProviderRuntimeError> {
        if !self.admission.supports_steering {
            return Err(ExternalProviderRuntimeError::UnsupportedSteering);
        }
        let capabilities = self.capability_report(&provider_session_id).await;
        let prompt = ProviderPromptContent::new(blocks, &capabilities)?;
        let (reply, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(ProviderCommand::Steer {
                provider_session_id,
                input_id,
                prompt,
                reply,
            })
            .await
            .map_err(|_| self.retired_operation_error())?;
        result.await.map_err(|_| self.retired_operation_error())?
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
            .map_err(|_| self.retired_operation_error())?;
        result.await.map_err(|_| self.retired_operation_error())
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
            .map_err(|_| self.retired_operation_error())?;
        result.await.map_err(|_| self.retired_operation_error())?
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
