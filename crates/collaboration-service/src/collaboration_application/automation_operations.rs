//! Automation: configuration, runs and instructions. Schedules and automation inspection extend
//! the same handle in `schedule_operations` and `automation_inspection_operations`; wakes have
//! their own handle in `wake_operations`.
//!
//! Each sub-area fails with its existing typed payload (`ConfigurationFailure`, `RunFailure`,
//! `InstructionFailure`, `ScheduleFailure`, `AutomationInspectionFailure`).
use super::{CollaborationRejection, CollaborationRejectionReason, PublishedRejection};
use crate::ServiceIdentity;
use automation_storage::{
    AutomationStore, InstructionUpdate, StorageError, SummaryRecoveryAction, SummaryRecoveryRequest,
};
use collaboration_protocol::{
    AutomationConfiguration, AutomationConfigureRequest, AutomationInspectionFailure,
    AutomationInspectionFailureKind, AutomationStatus, CodexGeneration, ConfigurationFailure,
    ConfigurationFailureKind, ConfigurationFileState, ConfigurationNextAction, EndpointRef,
    InstructionCreateParams, InstructionFailure, InstructionFailureKind, InstructionId,
    InstructionNextAction, InstructionShowParams, InstructionSnapshot, InstructionStage,
    InstructionUpdateParams, LocalMutationEvidence, LocalMutationState, OperationId, RunFailure,
    RunFailureKind, RunFailureStage, RunId, RunNextAction, RunRecoveryRequest, RunShowRequest,
    RunSnapshot, ScheduleFailure, ScheduleFailureKind, SessionRef,
};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Automation operations over automation storage, the Host's configuration and scheduled runs.
pub struct AutomationOperations<'service> {
    pub(super) identity: &'service ServiceIdentity,
}

impl<'service> AutomationOperations<'service> {
    pub(crate) fn new(identity: &'service ServiceIdentity) -> Self {
        Self { identity }
    }

    pub(super) fn store(&self) -> Option<&'service Arc<Mutex<AutomationStore>>> {
        self.identity.automation.as_ref()
    }

    /// Storage availability, the current configuration and the earliest retained event cursor.
    pub async fn automation_status(&self) -> AutomationStatus {
        let store = self.store();
        let floor = if let Some(store) = store {
            store
                .lock()
                .await
                .automation_event_floor(chrono::Utc::now().timestamp_millis())
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let cursor = floor.and_then(|(sequence, observed)| {
            serde_json::to_string(&(
                1_u8,
                &self.identity.service_id,
                "automation-events",
                sequence.saturating_sub(1),
                observed,
            ))
            .ok()
        });
        AutomationStatus {
            storage_available: store.is_some(),
            configuration: self.identity.configuration.current().await,
            earliest_retained_event_cursor: cursor,
        }
    }

    /// Replaces the automation timeouts through the Host, which owns the configuration file.
    pub async fn automation_configure(
        &self,
        request: AutomationConfigureRequest,
    ) -> Result<AutomationConfiguration, ConfigurationFailure> {
        let Some(backend) = self.identity.configuration_backend.as_ref() else {
            return Err(ConfigurationFailure {
                kind: ConfigurationFailureKind::AutomationUnavailable,
                message: "Host configuration backend unavailable; no file replacement dispatched."
                    .into(),
                operation_id: Some(request.operation_id),
                file_state: ConfigurationFileState::NotReplaced,
                next_action: ConfigurationNextAction::RetryLater,
            });
        };
        backend.configure(request).await
    }

    /// Shows one Run.
    pub async fn run_show(&self, request: RunShowRequest) -> Result<RunSnapshot, RunFailure> {
        let mut context = run_failure_context(None, Some(request.run_id.clone()));
        let store = self.run_store(&mut context)?;
        let read = store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                &request.run_id,
            )
            .await;
        project_run(read, context, RunAccess::Inspect)
    }

    /// Retries a blocked or required summary on the same Run, under the current configuration.
    pub async fn run_summary_retry(
        &self,
        request: RunRecoveryRequest,
    ) -> Result<RunSnapshot, RunFailure> {
        let mut context = run_failure_context(
            Some(request.operation_id.clone()),
            Some(request.run_id.clone()),
        );
        let store = self.run_store(&mut context)?;
        let lease = self.identity.configuration.admission_lease().await;
        let Some(configuration) = lease.configuration() else {
            context.kind = RunFailureKind::AutomationUnavailable;
            context.message =
                "Configuration reconciliation is pending; summary retry has not been admitted."
                    .into();
            return Err(context);
        };
        let action = SummaryRecoveryAction::Retry {
            timeout_seconds: u32::from(configuration.summary_timeout_seconds),
        };
        // The admission lease holds the configuration steady until the Run is projected.
        let recovered = recover_summary(store, request, action).await;
        let projected = project_run(recovered, context, RunAccess::Recover);
        drop(lease);
        projected
    }

    /// Skips a blocked or required summary; the Run completes without one.
    pub async fn run_summary_skip(
        &self,
        request: RunRecoveryRequest,
    ) -> Result<RunSnapshot, RunFailure> {
        let mut context = run_failure_context(
            Some(request.operation_id.clone()),
            Some(request.run_id.clone()),
        );
        let store = self.run_store(&mut context)?;
        let recovered = recover_summary(store, request, SummaryRecoveryAction::Skip).await;
        project_run(recovered, context, RunAccess::Recover)
    }

    fn run_store(
        &self,
        context: &mut RunFailure,
    ) -> Result<&'service Arc<Mutex<AutomationStore>>, RunFailure> {
        self.store().ok_or_else(|| {
            context.kind = RunFailureKind::AutomationUnavailable;
            context.message = "Automation storage unavailable; no Run mutation dispatched.".into();
            context.next_action = RunNextAction::RetryLater;
            context.clone()
        })
    }

    /// Creates an instruction document, or replays the one this operation created.
    pub async fn instruction_create(
        &self,
        request: InstructionCreateParams,
    ) -> Result<InstructionSnapshot, InstructionFailure> {
        let mut context = InstructionContext::mutation(Some(request.operation_id.clone()), None);
        let store = self.instruction_store(&context)?;
        let now = chrono::Utc::now().timestamp_millis();
        let created = store
            .lock()
            .await
            .create_instruction(&request.operation_id, &request.text, now)
            .await;
        project_instruction(created, &mut context)
    }

    /// Replaces an instruction's text when its current revision is the expected one.
    pub async fn instruction_update(
        &self,
        request: InstructionUpdateParams,
    ) -> Result<InstructionSnapshot, InstructionFailure> {
        let mut context = InstructionContext::mutation(
            Some(request.operation_id.clone()),
            Some(request.instruction_id.clone()),
        );
        let store = self.instruction_store(&context)?;
        let now = chrono::Utc::now().timestamp_millis();
        let updated = store
            .lock()
            .await
            .update_instruction(&InstructionUpdate {
                operation_id: request.operation_id,
                instruction_id: request.instruction_id,
                expected_revision_id: request.expected_revision_id,
                text: request.text,
                now_ms: now,
            })
            .await;
        project_instruction(updated, &mut context)
    }

    /// Shows an instruction's current revision.
    pub async fn instruction_show(
        &self,
        request: InstructionShowParams,
    ) -> Result<InstructionSnapshot, InstructionFailure> {
        let mut context = InstructionContext {
            operation_id: None,
            instruction_id: Some(request.instruction_id.clone()),
            mutation: false,
            current_revision_id: None,
        };
        let store = self.instruction_store(&context)?;
        let read = store
            .lock()
            .await
            .read_instruction(&request.instruction_id)
            .await;
        project_instruction(read, &mut context)
    }

    fn instruction_store(
        &self,
        context: &InstructionContext,
    ) -> Result<&'service Arc<Mutex<AutomationStore>>, InstructionFailure> {
        self.store().ok_or_else(|| {
            instruction_failure(
                context.clone(),
                InstructionFailureReason {
                    kind: InstructionFailureKind::AutomationUnavailable,
                    stage: InstructionStage::Storage,
                    message: "Automation storage is unavailable; no instruction mutation was dispatched.",
                    mutation: LocalMutationState::None,
                },
            )
        })
    }
}

/// The Run failure a request starts from: an invalid-request failure naming its identities.
pub(crate) fn run_failure_context(
    operation_id: Option<OperationId>,
    run_id: Option<RunId>,
) -> RunFailure {
    RunFailure {
        kind: RunFailureKind::InvalidField,
        stage: RunFailureStage::Validation,
        message: "Provide exact Run and operation identities using the published request fields."
            .into(),
        operation_id,
        run_id,
        effects: LocalMutationEvidence::Local {
            mutation: LocalMutationState::None,
        },
        next_action: RunNextAction::CorrectRequest,
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RunAccess {
    Inspect,
    Recover,
}

type StoredRun = agent_automation::RunRecord<
    SessionRef,
    EndpointRef,
    CodexGeneration,
    crate::stored_run_receipt::StoredRunReceipt,
>;

async fn recover_summary(
    store: &Arc<Mutex<AutomationStore>>,
    request: RunRecoveryRequest,
    action: SummaryRecoveryAction,
) -> Result<StoredRun, StorageError> {
    store
        .lock()
        .await
        .recover_summary::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
            &SummaryRecoveryRequest {
                operation_id: request.operation_id,
                run_id: request.run_id,
                action,
                now_ms: chrono::Utc::now().timestamp_millis(),
            },
        )
        .await
}

fn project_run(
    result: Result<StoredRun, StorageError>,
    mut context: RunFailure,
    access: RunAccess,
) -> Result<RunSnapshot, RunFailure> {
    let inspect = access == RunAccess::Inspect;
    match result {
        Ok(record) => crate::run_projection::snapshot(record).map_err(|()| {
            context.kind = RunFailureKind::InvalidRecord;
            context.stage = RunFailureStage::Inspection;
            context.message =
                "Stored Run cannot be projected consistently; inspect it before retrying.".into();
            context.next_action = RunNextAction::InspectRun;
            if !inspect {
                context.effects = LocalMutationEvidence::Local {
                    mutation: LocalMutationState::Committed,
                };
            }
            context
        }),
        Err(error) => {
            context.stage = if inspect {
                RunFailureStage::Inspection
            } else {
                RunFailureStage::Recovery
            };
            context.kind = match error {
                StorageError::OperationConflict => RunFailureKind::OperationConflict,
                StorageError::Database(_) => RunFailureKind::AutomationUnavailable,
                _ if inspect => RunFailureKind::ResourceNotFound,
                _ => RunFailureKind::RecoveryNotAllowed,
            };
            context.message="Run recovery requires a blocked/required summary, unchanged attempt identity and confirmed cessation or proven non-submission. Inspect current Run and attempt before retry or skip.".into();
            if matches!(error, StorageError::Database(_)) && !inspect {
                context.effects = LocalMutationEvidence::Local {
                    mutation: LocalMutationState::Unknown,
                };
                context.next_action = RunNextAction::InspectOperation;
            } else {
                context.next_action = RunNextAction::InspectRun;
            }
            Err(context)
        }
    }
}

/// The identities and mutation flag an instruction failure reports.
#[derive(Clone)]
pub(crate) struct InstructionContext {
    pub(crate) operation_id: Option<OperationId>,
    pub(crate) instruction_id: Option<InstructionId>,
    pub(crate) mutation: bool,
    pub(crate) current_revision_id: Option<collaboration_protocol::RevisionId>,
}

impl InstructionContext {
    fn mutation(operation_id: Option<OperationId>, instruction_id: Option<InstructionId>) -> Self {
        Self {
            operation_id,
            instruction_id,
            mutation: true,
            current_revision_id: None,
        }
    }
}

pub(crate) struct InstructionFailureReason<'a> {
    pub(crate) kind: InstructionFailureKind,
    pub(crate) stage: InstructionStage,
    pub(crate) message: &'a str,
    pub(crate) mutation: LocalMutationState,
}

/// An instruction failure with the next action its kind and context imply.
pub(crate) fn instruction_failure(
    context: InstructionContext,
    reason: InstructionFailureReason<'_>,
) -> InstructionFailure {
    let InstructionFailureReason {
        kind,
        stage,
        message,
        mutation,
    } = reason;
    let next_action = match kind {
        InstructionFailureKind::InvalidField => InstructionNextAction::CorrectRequest,
        InstructionFailureKind::RevisionConflict | InstructionFailureKind::ResourceNotFound => {
            InstructionNextAction::InspectInstruction
        }
        InstructionFailureKind::Overloaded => InstructionNextAction::RetryLater,
        _ if context.operation_id.is_some() => InstructionNextAction::InspectOperation,
        _ => InstructionNextAction::RetryLater,
    };
    InstructionFailure {
        kind,
        stage,
        message: message.into(),
        operation_id: context.operation_id,
        instruction_id: context.instruction_id,
        current_revision_id: context.current_revision_id,
        effects: LocalMutationEvidence::Local { mutation },
        next_action,
    }
}

fn project_instruction(
    result: Result<agent_automation::InstructionDocument, StorageError>,
    context: &mut InstructionContext,
) -> Result<InstructionSnapshot, InstructionFailure> {
    match result {
        Ok(document) => crate::instruction_projection::snapshot(document).map_err(|()| {
            let mutation = if context.mutation {
                LocalMutationState::Committed
            } else {
                LocalMutationState::None
            };
            instruction_failure(
                context.clone(),
                InstructionFailureReason {
                    kind: InstructionFailureKind::InvalidRecord,
                    stage: InstructionStage::Inspection,
                    message: "Instruction outcome exists but its stored timestamps are invalid; inspect the operation before retrying.",
                    mutation,
                },
            )
        }),
        Err(error) => {
            if let StorageError::RevisionConflict {
                current_revision_id,
            } = &error
            {
                context.current_revision_id = Some(current_revision_id.clone());
            }
            let (kind, message) = match &error {
                StorageError::OperationConflict => (
                    InstructionFailureKind::OperationConflict,
                    "Operation identity belongs to a different request; inspect that operation or choose a new identity for new work.",
                ),
                StorageError::RevisionConflict { .. } => (
                    InstructionFailureKind::RevisionConflict,
                    "Instruction changed; read its current revision before editing.",
                ),
                StorageError::InstructionNotFound => (
                    InstructionFailureKind::ResourceNotFound,
                    "Instruction was not found in this service; verify its identity.",
                ),
                StorageError::Database(_) => (
                    InstructionFailureKind::AutomationUnavailable,
                    "Instruction storage request failed; a mutation may have committed. Inspect or replay the same operation identity, never create a new one blindly.",
                ),
                _ => (
                    InstructionFailureKind::InvalidRecord,
                    "Instruction storage is inconsistent; inspect the stored operation and record.",
                ),
            };
            let mutation = if context.mutation && matches!(error, StorageError::Database(_)) {
                LocalMutationState::Unknown
            } else {
                LocalMutationState::None
            };
            Err(instruction_failure(
                context.clone(),
                InstructionFailureReason {
                    kind,
                    stage: InstructionStage::Storage,
                    message,
                    mutation,
                },
            ))
        }
    }
}

impl CollaborationRejection for ConfigurationFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.kind {
            ConfigurationFailureKind::InvalidField => {
                Some(CollaborationRejectionReason::InvalidShape)
            }
            ConfigurationFailureKind::OperationConflict => {
                Some(CollaborationRejectionReason::ConflictingRequest)
            }
            ConfigurationFailureKind::AutomationUnavailable
            | ConfigurationFailureKind::OutcomeUnknown => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        PublishedRejection::typed(
            PublishedRejection::OPERATION_FAILED,
            "Automation configuration failed",
            self,
        )
    }
}

impl CollaborationRejection for RunFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.kind {
            RunFailureKind::Overloaded => Some(CollaborationRejectionReason::Overloaded),
            RunFailureKind::InvalidField => Some(CollaborationRejectionReason::InvalidShape),
            RunFailureKind::OperationConflict => {
                Some(CollaborationRejectionReason::ConflictingRequest)
            }
            RunFailureKind::ResourceNotFound
            | RunFailureKind::RecoveryNotAllowed
            | RunFailureKind::AutomationUnavailable
            | RunFailureKind::InvalidRecord => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        PublishedRejection::typed(
            PublishedRejection::OPERATION_FAILED,
            "Run request failed",
            self,
        )
    }
}

impl CollaborationRejection for InstructionFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.kind {
            InstructionFailureKind::Overloaded => Some(CollaborationRejectionReason::Overloaded),
            InstructionFailureKind::InvalidField => {
                Some(CollaborationRejectionReason::InvalidShape)
            }
            InstructionFailureKind::OperationConflict => {
                Some(CollaborationRejectionReason::ConflictingRequest)
            }
            InstructionFailureKind::RevisionConflict => {
                Some(CollaborationRejectionReason::StaleRevision)
            }
            InstructionFailureKind::ResourceNotFound
            | InstructionFailureKind::AutomationUnavailable
            | InstructionFailureKind::InvalidRecord => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        PublishedRejection::typed(
            PublishedRejection::OPERATION_FAILED,
            "Instruction operation failed",
            self,
        )
    }
}

impl CollaborationRejection for ScheduleFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.kind {
            ScheduleFailureKind::Overloaded => Some(CollaborationRejectionReason::Overloaded),
            ScheduleFailureKind::InvalidField => Some(CollaborationRejectionReason::InvalidShape),
            ScheduleFailureKind::OperationConflict => {
                Some(CollaborationRejectionReason::ConflictingRequest)
            }
            ScheduleFailureKind::ChangeConflict => {
                Some(CollaborationRejectionReason::StaleRevision)
            }
            ScheduleFailureKind::OutcomeUnknown
            | ScheduleFailureKind::ResourceNotFound
            | ScheduleFailureKind::AutomationUnavailable
            | ScheduleFailureKind::InvalidRecord
            | ScheduleFailureKind::UnsupportedCapability
            | ScheduleFailureKind::OwnershipConflict
            | ScheduleFailureKind::InstructionConflict => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        PublishedRejection::typed(
            PublishedRejection::OPERATION_FAILED,
            "Schedule operation failed",
            self,
        )
    }
}

impl CollaborationRejection for AutomationInspectionFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.kind {
            AutomationInspectionFailureKind::Overloaded => {
                Some(CollaborationRejectionReason::Overloaded)
            }
            AutomationInspectionFailureKind::InvalidField => {
                Some(CollaborationRejectionReason::InvalidShape)
            }
            AutomationInspectionFailureKind::ResourceNotFound
            | AutomationInspectionFailureKind::AutomationUnavailable
            | AutomationInspectionFailureKind::InvalidRecord
            | AutomationInspectionFailureKind::HistoryExpired => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        PublishedRejection::typed(
            PublishedRejection::OPERATION_FAILED,
            "Automation inspection failed",
            self,
        )
    }
}
