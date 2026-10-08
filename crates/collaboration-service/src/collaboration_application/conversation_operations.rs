//! Provider conversations: Host-owned operations on external provider sessions, plus the
//! inspection and recovery of recorded Codex ACP operations.
use super::{CollaborationRejection, CollaborationRejectionReason};
use crate::{ProviderConversationBackend, ServiceIdentity};
use collaboration_protocol::{
    CodexGeneration, ConversationAdmissionState, ConversationBindingIdentity,
    ConversationCancelRequest, ConversationCloseRequest, ConversationCreateRequest,
    ConversationLoadRequest, ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationFailureStage, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, ConversationOperationSnapshot,
    ConversationOperationSubmission, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, ConversationOperationWaitResult,
    ConversationOutputUnavailableReason, ConversationPromptRequest, ConversationResumeRequest,
    EndpointAvailability, EndpointRef, NonEmptyText, OperationId, ProviderIdentity,
    ProviderInspectFailure, ProviderInspectFailureKind, ProviderOperationEffect,
    ProviderOperationStage, ProviderReconciliationState, ProviderSessionInspectRequest,
    ProviderSessionInspectResult, ProviderSettingsAcceptRequest, ProviderSettingsFailure,
    ProviderSettingsFailureKind, ProviderSettingsResult, ProviderSettingsSetRequest, SessionRef,
};

/// Conversation operations over the Host's provider backend and Codex operation records.
pub struct ConversationOperations<'service> {
    identity: &'service ServiceIdentity,
}

/// Submits one admitted operation to the backend with the binding's current generation.
macro_rules! submitted_operations {
    ($($(#[$documentation:meta])* $operation:ident($request:ty) => $backend_method:ident;)*) => {
        $(
            $(#[$documentation])*
            pub async fn $operation(
                &self,
                mut request: $request,
            ) -> Result<ConversationOperationSubmission, ConversationFailure> {
                let admission = self
                    .admit_submission(SubmissionCheck {
                        operation_id: &request.operation_id,
                        target: Some(&request.target),
                        endpoint: &request.target.endpoint,
                        actors: [&request.requested_by, &request.approver],
                        expected_generation: request.generation.as_ref(),
                    })
                    .await?;
                let (backend, generation) = match admission {
                    SubmissionAdmission::Existing(submission) => return Ok(*submission),
                    SubmissionAdmission::Admitted { backend, generation } => (backend, generation),
                };
                request.generation = Some(generation);
                backend
                    .$backend_method(request)
                    .await
                    .map_err(ConversationFailure::Operation)
            }
        )*
    };
}

impl<'service> ConversationOperations<'service> {
    pub(crate) fn new(identity: &'service ServiceIdentity) -> Self {
        Self { identity }
    }

    fn backend(&self) -> Option<&'service dyn ProviderConversationBackend> {
        self.identity.provider_conversations.as_deref()
    }

    /// Creates a provider session on the endpoint's active binding.
    pub async fn conversation_create(
        &self,
        mut request: ConversationCreateRequest,
    ) -> Result<ConversationOperationSubmission, ConversationFailure> {
        let admission = self
            .admit_submission(SubmissionCheck {
                operation_id: &request.operation_id,
                target: None,
                endpoint: &request.endpoint,
                actors: [&request.created_by, &request.approver],
                expected_generation: request.generation.as_ref(),
            })
            .await?;
        let (backend, generation) = match admission {
            SubmissionAdmission::Existing(submission) => return Ok(*submission),
            SubmissionAdmission::Admitted {
                backend,
                generation,
            } => (backend, generation),
        };
        request.generation = Some(generation);
        backend
            .create(request)
            .await
            .map_err(ConversationFailure::Operation)
    }

    submitted_operations! {
        /// Loads an existing provider session into the binding.
        conversation_load(ConversationLoadRequest) => load;
        /// Resumes a provider session.
        conversation_resume(ConversationResumeRequest) => resume;
        /// Closes a provider session.
        conversation_close(ConversationCloseRequest) => close;
        /// Prompts a provider session.
        conversation_prompt(ConversationPromptRequest) => prompt;
        /// Cancels the provider session's running prompt.
        conversation_cancel(ConversationCancelRequest) => cancel;
    }

    /// Changes one advertised provider setting.
    pub async fn settings_set(
        &self,
        request: ProviderSettingsSetRequest,
    ) -> Result<ProviderSettingsResult, ProviderSettingsFailure> {
        let Some(backend) = self.backend() else {
            return Err(settings_unavailable(request.target));
        };
        backend.settings_set(request).await
    }

    /// Accepts the provider's current settings without calling the agent.
    pub async fn settings_accept(
        &self,
        request: ProviderSettingsAcceptRequest,
    ) -> Result<ProviderSettingsResult, ProviderSettingsFailure> {
        let Some(backend) = self.backend() else {
            return Err(settings_unavailable(request.target));
        };
        backend.settings_accept(request).await
    }

    /// Reads a provider session's live state.
    pub async fn provider_session_inspect(
        &self,
        request: ProviderSessionInspectRequest,
    ) -> Result<ProviderSessionInspectResult, ProviderInspectFailure> {
        let Some(backend) = self.backend() else {
            return Err(ProviderInspectFailure {
                kind: ProviderInspectFailureKind::Unavailable,
                stage: None,
                target: Some(request.target),
                message: "provider Session inspection is unavailable".into(),
            });
        };
        backend.inspect_session(request).await
    }

    /// Shows one operation, whether a recorded Codex operation or a provider operation.
    pub async fn operation_show(
        &self,
        request: ConversationOperationShowRequest,
    ) -> Result<ConversationOperationSnapshot, ConversationFailure> {
        if let Some(record) = self.codex_record(&request.operation_id).await? {
            return codex_snapshot(record);
        }
        let backend = self.read_backend(&request.operation_id)?;
        backend
            .show(request)
            .await
            .map_err(ConversationFailure::Operation)
    }

    /// Waits up to `timeoutSeconds` for an operation to settle.
    pub async fn operation_wait(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> Result<ConversationOperationWaitResult, ConversationFailure> {
        if self.codex_record(&request.operation_id).await?.is_some() {
            return self.codex_wait(&request).await;
        }
        let backend = self.read_backend(&request.operation_id)?;
        backend
            .wait(request)
            .await
            .map_err(ConversationFailure::Operation)
    }

    /// Reconciles an operation whose outcome is unresolved.
    pub async fn operation_reconcile(
        &self,
        request: ConversationOperationReconcileRequest,
    ) -> Result<ConversationOperationSnapshot, ConversationFailure> {
        if self.codex_record(&request.operation_id).await?.is_some() {
            return self.codex_reconcile(&request.operation_id).await;
        }
        let backend = self.read_backend(&request.operation_id)?;
        backend
            .reconcile(request)
            .await
            .map_err(ConversationFailure::Operation)
    }

    fn read_backend(
        &self,
        operation_id: &OperationId,
    ) -> Result<&'service dyn ProviderConversationBackend, ConversationFailure> {
        self.backend()
            .ok_or_else(|| local_failure(LocalFailure::Unavailable, operation_id.clone(), None))
    }

    async fn admit_submission(
        &self,
        check: SubmissionCheck<'_>,
    ) -> Result<SubmissionAdmission<'service>, ConversationFailure> {
        let operation_id = check.operation_id;
        let target = check.target.cloned();
        let Some(backend) = self.backend() else {
            return Err(self.unavailable_provider(operation_id.clone(), target, check.endpoint));
        };
        if check.endpoint.service_id != self.identity.service_id
            || !check.actors.iter().all(|actor| self.actor_is_local(actor))
        {
            return Err(local_failure(
                LocalFailure::InvalidIdentity,
                operation_id.clone(),
                target,
            ));
        }
        match backend.lookup_existing(operation_id.clone()).await {
            Ok(Some(operation)) => {
                return Ok(SubmissionAdmission::Existing(Box::new(
                    ConversationOperationSubmission {
                        admission: ConversationAdmissionState::Existing,
                        operation,
                    },
                )));
            }
            Ok(None) => {}
            Err(failure) => return Err(ConversationFailure::Operation(failure)),
        }
        if !matches!(self.provider_unavailability(check.endpoint), Ok(None)) {
            return Err(self.unavailable_provider(operation_id.clone(), target, check.endpoint));
        }
        let Some(binding) = backend.binding(check.endpoint) else {
            return Err(local_failure(
                LocalFailure::InvalidBinding,
                operation_id.clone(),
                target,
            ));
        };
        if binding.endpoint != *check.endpoint {
            return Err(local_failure(
                LocalFailure::InvalidIdentity,
                operation_id.clone(),
                target,
            ));
        }
        if check
            .expected_generation
            .is_some_and(|expected| expected != &binding.generation)
        {
            return Err(local_failure(
                LocalFailure::StaleGeneration,
                operation_id.clone(),
                target,
            ));
        }
        Ok(SubmissionAdmission::Admitted {
            backend,
            generation: binding.generation,
        })
    }

    fn actor_is_local(&self, actor: &ProviderIdentity) -> bool {
        actor
            .session()
            .is_none_or(|session| session.endpoint.service_id == self.identity.service_id)
    }

    fn provider_unavailability(
        &self,
        endpoint: &EndpointRef,
    ) -> Result<Option<EndpointAvailability>, std::io::Error> {
        let description = self.identity.endpoint_directory().read_endpoint(endpoint)?;
        Ok(
            description.and_then(|description| match description.availability {
                unavailable @ EndpointAvailability::Unavailable { .. } => Some(unavailable),
                EndpointAvailability::Available { .. } | EndpointAvailability::Unprobed => None,
            }),
        )
    }

    fn unavailable_provider(
        &self,
        operation_id: OperationId,
        target: Option<SessionRef>,
        endpoint: &EndpointRef,
    ) -> ConversationFailure {
        let availability = self.provider_unavailability(endpoint).ok().flatten();
        let endpoint_id = String::from(endpoint.endpoint_id.clone());
        let message = match &availability {
            Some(EndpointAvailability::Unavailable { reason, .. }) => {
                format!(
                    "provider conversation endpoint {endpoint_id} unavailable: {}",
                    String::from(reason.clone())
                )
            }
            _ => format!("provider conversation endpoint {endpoint_id} unavailable"),
        };
        let message = NonEmptyText::try_from(message).or_else(|_| {
            NonEmptyText::try_from(format!("provider endpoint {endpoint_id} unavailable"))
        });
        let Ok(message) = message else {
            return ConversationFailure::UndescribableFailure;
        };
        ConversationFailure::Operation(ConversationOperationFailure {
            kind: ConversationOperationFailureKind::Unavailable,
            stage: ConversationOperationFailureStage::Binding,
            effect: ProviderOperationEffect::None,
            message,
            operation_id: Some(operation_id),
            invalid_setting: None,
            provider_code: None,
            target,
            endpoint: Some(endpoint.clone()),
            availability,
        })
    }

    /// The recorded Codex ACP operation with this identity, if one exists.
    async fn codex_record(
        &self,
        operation_id: &OperationId,
    ) -> Result<Option<crate::ProviderOperationRecord>, ConversationFailure> {
        let Some(store) = self.identity.provider_operations.as_ref() else {
            return Ok(None);
        };
        let record = store
            .lock()
            .await
            .inspect(operation_id)
            .await
            .map_err(|_| local_failure(LocalFailure::Unavailable, operation_id.clone(), None))?;
        Ok(record.filter(|record| {
            matches!(
                &record.binding,
                ConversationBindingIdentity::CodexAcp { .. }
            )
        }))
    }

    async fn codex_wait(
        &self,
        request: &ConversationOperationWaitRequest,
    ) -> Result<ConversationOperationWaitResult, ConversationFailure> {
        let operation_id = &request.operation_id;
        let Some(recorder) = self.identity.codex_conversation_recorder.as_ref() else {
            return Err(local_failure(
                LocalFailure::Unavailable,
                operation_id.clone(),
                None,
            ));
        };
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(u64::from(u32::from(request.timeout_seconds)));
        loop {
            let mut changed = Box::pin(recorder.changed().notified_owned());
            changed.as_mut().enable();
            let Some(record) = self.codex_record(operation_id).await? else {
                return Err(local_failure(
                    LocalFailure::InvalidBinding,
                    operation_id.clone(),
                    None,
                ));
            };
            let terminal = record.stage == ProviderOperationStage::Terminal;
            if terminal || tokio::time::Instant::now() >= deadline {
                let output = if terminal {
                    ConversationOperationWaitOutput::OutputUnavailable {
                        reason: ConversationOutputUnavailableReason::NotRetained,
                    }
                } else {
                    ConversationOperationWaitOutput::Pending
                };
                let operation = codex_snapshot(record)?;
                return Ok(ConversationOperationWaitResult { operation, output });
            }
            tokio::select! {
                () = &mut changed => {}
                () = tokio::time::sleep_until(deadline) => {}
            }
        }
    }

    async fn codex_reconcile(
        &self,
        operation_id: &OperationId,
    ) -> Result<ConversationOperationSnapshot, ConversationFailure> {
        let unavailable = || local_failure(LocalFailure::Unavailable, operation_id.clone(), None);
        let Some(store) = self.identity.provider_operations.as_ref() else {
            return Err(unavailable());
        };
        let active = self
            .identity
            .codex_conversation_recorder
            .as_ref()
            .is_some_and(|recorder| recorder.is_active(operation_id));
        let mut store = store.lock().await;
        let record = match store.inspect(operation_id).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                return Err(local_failure(
                    LocalFailure::InvalidBinding,
                    operation_id.clone(),
                    None,
                ));
            }
            Err(_) => return Err(unavailable()),
        };
        let updated = if record.target.is_some()
            && record.reconciliation_state != ProviderReconciliationState::Confirmed
        {
            store
                .confirm_recorded_target(operation_id, chrono::Utc::now().timestamp_millis())
                .await
        } else if !active && record.reconciliation_state == ProviderReconciliationState::Unresolved
        {
            store
                .record_not_reconcilable(operation_id, chrono::Utc::now().timestamp_millis())
                .await
        } else {
            Ok(record)
        };
        match updated {
            Ok(record) => codex_snapshot(record),
            Err(_) => Err(unavailable()),
        }
    }
}

struct SubmissionCheck<'request> {
    operation_id: &'request OperationId,
    /// The session the operation acts on; `None` when it creates one.
    target: Option<&'request SessionRef>,
    endpoint: &'request EndpointRef,
    actors: [&'request ProviderIdentity; 2],
    expected_generation: Option<&'request CodexGeneration>,
}

enum SubmissionAdmission<'service> {
    /// The operation identity was already admitted; its current state is the answer.
    Existing(Box<ConversationOperationSubmission>),
    Admitted {
        backend: &'service dyn ProviderConversationBackend,
        generation: CodexGeneration,
    },
}

/// Why a conversation operation failed.
#[derive(Clone, Debug, thiserror::Error)]
#[expect(
    clippy::large_enum_variant,
    reason = "the typed operation failure is the common case; the unit variants are rare"
)]
pub enum ConversationFailure {
    /// The typed failure the provider path or local admission reported.
    #[error("{}", String::from(.0.message.clone()))]
    Operation(ConversationOperationFailure),
    /// A recorded Codex operation could not be projected.
    #[error("Invalid stored conversation operation")]
    InvalidStoredOperation,
    /// The failure's own description could not be represented.
    #[error("Internal error")]
    UndescribableFailure,
}

impl CollaborationRejection for ConversationFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        let Self::Operation(failure) = self else {
            return None;
        };
        match failure.kind {
            ConversationOperationFailureKind::Overloaded => {
                Some(CollaborationRejectionReason::Overloaded)
            }
            ConversationOperationFailureKind::InvalidRequest
            | ConversationOperationFailureKind::InvalidSetting
            | ConversationOperationFailureKind::SettingsUnresolved
            | ConversationOperationFailureKind::UnsupportedCapability
            | ConversationOperationFailureKind::AuthenticationRequired
            | ConversationOperationFailureKind::PermissionRejected
            | ConversationOperationFailureKind::Busy
            | ConversationOperationFailureKind::NotFound
            | ConversationOperationFailureKind::ProviderSessionNotFound
            | ConversationOperationFailureKind::StaleGeneration
            | ConversationOperationFailureKind::Unavailable
            | ConversationOperationFailureKind::ProviderRejected
            | ConversationOperationFailureKind::ProtocolViolation
            | ConversationOperationFailureKind::OutcomeUnknown => None,
        }
    }
}

impl CollaborationRejection for ProviderSettingsFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.kind {
            ProviderSettingsFailureKind::Overloaded => {
                Some(CollaborationRejectionReason::Overloaded)
            }
            ProviderSettingsFailureKind::WrongActor
            | ProviderSettingsFailureKind::NotFound
            | ProviderSettingsFailureKind::Busy
            | ProviderSettingsFailureKind::InvalidSetting
            | ProviderSettingsFailureKind::ProviderRejected
            | ProviderSettingsFailureKind::OutcomeUnknown
            | ProviderSettingsFailureKind::Unavailable => None,
        }
    }
}

impl CollaborationRejection for ProviderInspectFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.kind {
            ProviderInspectFailureKind::Overloaded => {
                Some(CollaborationRejectionReason::Overloaded)
            }
            ProviderInspectFailureKind::NotFound | ProviderInspectFailureKind::Unavailable => None,
        }
    }
}

enum LocalFailure {
    InvalidIdentity,
    InvalidBinding,
    StaleGeneration,
    Unavailable,
}

fn local_failure(
    failure: LocalFailure,
    operation_id: OperationId,
    target: Option<SessionRef>,
) -> ConversationFailure {
    let (kind, stage, message) = match failure {
        LocalFailure::InvalidIdentity => (
            ConversationOperationFailureKind::InvalidRequest,
            ConversationOperationFailureStage::Validation,
            "provider conversation identity does not match the active binding",
        ),
        LocalFailure::InvalidBinding => (
            ConversationOperationFailureKind::InvalidRequest,
            ConversationOperationFailureStage::Binding,
            "provider conversation endpoint does not resolve to an active binding",
        ),
        LocalFailure::StaleGeneration => (
            ConversationOperationFailureKind::StaleGeneration,
            ConversationOperationFailureStage::Binding,
            "provider conversation generation is stale",
        ),
        LocalFailure::Unavailable => (
            ConversationOperationFailureKind::Unavailable,
            ConversationOperationFailureStage::Binding,
            "provider conversation backend unavailable",
        ),
    };
    let Ok(message) = NonEmptyText::try_from(message.to_owned()) else {
        return ConversationFailure::UndescribableFailure;
    };
    ConversationFailure::Operation(ConversationOperationFailure {
        kind,
        stage,
        effect: ProviderOperationEffect::None,
        message,
        operation_id: Some(operation_id),
        invalid_setting: None,
        provider_code: None,
        target,
        endpoint: None,
        availability: None,
    })
}

fn codex_snapshot(
    record: crate::ProviderOperationRecord,
) -> Result<ConversationOperationSnapshot, ConversationFailure> {
    crate::conversation_operation_snapshot(record)
        .map_err(|_| ConversationFailure::InvalidStoredOperation)
}

fn settings_unavailable(target: SessionRef) -> ProviderSettingsFailure {
    ProviderSettingsFailure {
        kind: ProviderSettingsFailureKind::Unavailable,
        stage: None,
        target: Some(target),
        message: "provider settings service is unavailable".into(),
        setting: None,
        value: None,
        advertised: Vec::new(),
    }
}
