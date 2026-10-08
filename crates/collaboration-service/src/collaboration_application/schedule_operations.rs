//! Schedules: local administration and portable packages, plus the explicit preparation that
//! allocates a native destination. Only preparation reaches a native route.
use super::{
    AutomationOperations, CollaborationRejection, CollaborationRejectionReason, PublishedRejection,
    ResultByteBudget,
};
use automation_storage::{
    AutomationStore, ScheduleCreate, ScheduleEdit, ScheduleInspection, ScheduleMutation,
    StorageError,
};
use collaboration_protocol::{
    ChangeId, EndpointRef, LocalMutationState, OperationId, ScheduleCreateRequest, ScheduleEffects,
    ScheduleEnableRequest, ScheduleExportResult, ScheduleFailure, ScheduleFailureKind,
    ScheduleFailureStage, ScheduleId, ScheduleImportRequest, ScheduleNextAction,
    SchedulePrepareRequest, ScheduleShowRequest, ScheduleSnapshot, ScheduleUpdateRequest,
    SessionRef,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

type ScheduleResult<TResult> = Result<TResult, ScheduleOperationFailure>;

impl<'service> AutomationOperations<'service> {
    fn schedule_store(
        &self,
        context: &ScheduleFailureContext,
    ) -> ScheduleResult<&'service Arc<Mutex<AutomationStore>>> {
        self.store().ok_or_else(|| {
            ScheduleOperationFailure::Operation(schedule_failure(
                context.clone(),
                StorageError::InvalidRecord,
                LocalMutationState::None,
                None,
            ))
        })
    }

    /// Creates a schedule once its activation is admitted.
    pub async fn schedule_create(
        &self,
        request: ScheduleCreateRequest,
    ) -> ScheduleResult<ScheduleSnapshot> {
        let context = ScheduleFailureContext::new(Some(request.operation_id.clone()), None);
        let store = self.schedule_store(&context)?;
        let Ok(definition) =
            serde_json::to_value(request.definition).and_then(serde_json::from_value)
        else {
            return Err(invalid_schedule_request(context));
        };
        self.validate_activation(store, &definition, None, &request.operation_id, &context)
            .await?;
        let created = store
            .lock()
            .await
            .create_schedule(&ScheduleCreate {
                operation_id: request.operation_id,
                definition,
                imported_continuity: agent_automation::ContinuityInput::None,
                now_ms: chrono::Utc::now().timestamp_millis(),
            })
            .await
            .map(|record| ScheduleInspection {
                record,
                active_run_id: None,
                waiting_run_id: None,
            });
        project_schedule(store, created, context, ScheduleAccess::Mutate).await
    }

    /// Shows one schedule with its active and waiting Runs.
    pub async fn schedule_show(
        &self,
        request: ScheduleShowRequest,
    ) -> ScheduleResult<ScheduleSnapshot> {
        let context = ScheduleFailureContext::new(None, Some(request.schedule_id.clone()));
        let store = self.schedule_store(&context)?;
        let inspected = store
            .lock()
            .await
            .inspect_schedule(&request.schedule_id)
            .await;
        project_schedule(store, inspected, context, ScheduleAccess::Inspect).await
    }

    /// Replaces a schedule's definition when its current change is the expected one.
    pub async fn schedule_update(
        &self,
        request: ScheduleUpdateRequest,
    ) -> ScheduleResult<ScheduleSnapshot> {
        let context = ScheduleFailureContext::new(
            Some(request.operation_id.clone()),
            Some(request.schedule_id.clone()),
        );
        let store = self.schedule_store(&context)?;
        let Ok(definition) =
            serde_json::to_value(request.definition).and_then(serde_json::from_value)
        else {
            return Err(invalid_schedule_request(context));
        };
        self.validate_activation(
            store,
            &definition,
            Some(&request.schedule_id),
            &request.operation_id,
            &context,
        )
        .await?;
        let mutated = store
            .lock()
            .await
            .mutate_schedule(&ScheduleMutation {
                operation_id: request.operation_id,
                schedule_id: request.schedule_id,
                edit: ScheduleEdit::Replace {
                    expected_change_id: request.expected_change_id,
                    definition,
                },
                now_ms: chrono::Utc::now().timestamp_millis(),
            })
            .await;
        project_schedule(store, mutated, context, ScheduleAccess::Mutate).await
    }

    /// Enables a schedule once its activation is admitted.
    pub async fn schedule_enable(
        &self,
        request: ScheduleEnableRequest,
    ) -> ScheduleResult<ScheduleSnapshot> {
        self.set_schedule_enabled(request, true).await
    }

    /// Disables a schedule; Runs already handed to a native thread are not stopped.
    pub async fn schedule_disable(
        &self,
        request: ScheduleEnableRequest,
    ) -> ScheduleResult<ScheduleSnapshot> {
        self.set_schedule_enabled(request, false).await
    }

    async fn set_schedule_enabled(
        &self,
        request: ScheduleEnableRequest,
        enabled: bool,
    ) -> ScheduleResult<ScheduleSnapshot> {
        let context = ScheduleFailureContext::new(
            Some(request.operation_id.clone()),
            Some(request.schedule_id.clone()),
        );
        let store = self.schedule_store(&context)?;
        if enabled {
            let current = store
                .lock()
                .await
                .inspect_schedule::<SessionRef, EndpointRef>(&request.schedule_id)
                .await;
            if let Ok(mut current) = current {
                current.record.definition.enabled = true;
                self.validate_activation(
                    store,
                    &current.record.definition,
                    Some(&request.schedule_id),
                    &request.operation_id,
                    &context,
                )
                .await?;
            }
        }
        let mutated = store
            .lock()
            .await
            .mutate_schedule(&ScheduleMutation {
                operation_id: request.operation_id,
                schedule_id: request.schedule_id,
                edit: ScheduleEdit::SetEnabled { enabled },
                now_ms: chrono::Utc::now().timestamp_millis(),
            })
            .await;
        project_schedule(store, mutated, context, ScheduleAccess::Mutate).await
    }

    async fn validate_activation(
        &self,
        store: &Arc<Mutex<AutomationStore>>,
        definition: &agent_automation::ScheduleDefinition<SessionRef, EndpointRef>,
        schedule_id: Option<&ScheduleId>,
        operation_id: &OperationId,
        context: &ScheduleFailureContext,
    ) -> ScheduleResult<()> {
        crate::schedule_activation::validate(
            store,
            crate::schedule_activation::ActivationRequest {
                definition,
                schedule_id,
                operation_id,
                service_id: &self.identity.service_id,
                execution: self.identity.scheduled_run_execution.as_ref(),
            },
        )
        .await
        .map_err(|error| {
            ScheduleOperationFailure::Operation(schedule_failure(
                context.clone(),
                error,
                LocalMutationState::None,
                None,
            ))
        })
    }

    /// Exports a schedule and its instruction as a portable package.
    ///
    /// The package must be importable again, so both this response and the largest request
    /// that would import it must fit `budget`'s response limit.
    pub async fn schedule_export(
        &self,
        request: ScheduleShowRequest,
        budget: ResultByteBudget,
    ) -> ScheduleResult<ScheduleExportResult> {
        let context = ScheduleFailureContext::new(None, Some(request.schedule_id.clone()));
        let store = self.schedule_store(&context)?;
        let operation_failure = |error| {
            ScheduleOperationFailure::Operation(schedule_failure(
                context.clone(),
                error,
                LocalMutationState::None,
                None,
            ))
        };
        let package = store
            .lock()
            .await
            .export_schedule::<SessionRef, EndpointRef>(&request.schedule_id)
            .await
            .map_err(operation_failure)?;
        // Encode once without a text-only cap, then measure the actual escaped response envelope.
        let encoded = agent_automation::encode_schedule_package(&package, usize::MAX)
            .map_err(|error| operation_failure(error.into()))?;
        // Request IDs allow 128 UTF-8 bytes; a control character needs six JSON bytes.
        // Reserve a maximally escaped ID and the larger import envelope before exporting.
        let import = json!({
            "jsonrpc":"2.0", "id":"\u{0001}".repeat(128), "method":"schedule/import",
            "params":{"operationId":"00000000-0000-7000-8000-000000000000","packageUtf8":encoded,"overwrite":false}
        });
        let import_bytes = serde_json::to_vec(&import)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        let result = ScheduleExportResult {
            package_utf8: encoded,
        };
        let Some(response_bytes) = budget.response_bytes(&result) else {
            return Err(operation_failure(StorageError::InvalidRecord));
        };
        let required_bytes = response_bytes.max(import_bytes);
        if required_bytes > budget.response_limit_bytes() {
            return Err(ScheduleOperationFailure::PackageTooLarge(
                ScheduleFailure::package_frame_limit(None, required_bytes),
            ));
        }
        Ok(result)
    }

    /// Imports a portable package; an existing schedule identity needs explicit overwrite.
    pub async fn schedule_import(
        &self,
        request: ScheduleImportRequest,
    ) -> ScheduleResult<ScheduleSnapshot> {
        let context = ScheduleFailureContext::new(Some(request.operation_id.clone()), None);
        let store = self.schedule_store(&context)?;
        let imported = store
            .lock()
            .await
            .import_schedule::<SessionRef, EndpointRef>(&automation_storage::ScheduleImport {
                operation_id: request.operation_id,
                package_utf8: &request.package_utf8,
                overwrite: request.overwrite,
                now_ms: chrono::Utc::now().timestamp_millis(),
            })
            .await;
        match imported {
            Ok(record) => crate::schedule_projection::snapshot(record).map_err(|()| {
                ScheduleOperationFailure::Operation(schedule_failure(
                    context,
                    StorageError::InvalidRecord,
                    LocalMutationState::Committed,
                    None,
                ))
            }),
            Err(error) => {
                let mutation = if matches!(error, StorageError::Database(_)) {
                    LocalMutationState::Unknown
                } else {
                    LocalMutationState::None
                };
                Err(ScheduleOperationFailure::Operation(schedule_failure(
                    context, error, mutation, None,
                )))
            }
        }
    }

    /// Prepares the schedule's reuse-thread destination; a retry returns the retained outcome.
    pub async fn schedule_prepare(
        &self,
        request: SchedulePrepareRequest,
    ) -> ScheduleResult<ScheduleSnapshot> {
        crate::schedule_preparation_dispatch::prepare_schedule(self.identity, request).await
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ScheduleAccess {
    Inspect,
    Mutate,
}

async fn project_schedule(
    store: &Arc<Mutex<AutomationStore>>,
    result: Result<ScheduleInspection<SessionRef, EndpointRef>, StorageError>,
    context: ScheduleFailureContext,
    access: ScheduleAccess,
) -> ScheduleResult<ScheduleSnapshot> {
    let mutating = access == ScheduleAccess::Mutate;
    match result {
        Ok(result) => crate::schedule_projection::snapshot(result).map_err(|()| {
            let mutation = if mutating {
                LocalMutationState::Committed
            } else {
                LocalMutationState::None
            };
            ScheduleOperationFailure::Operation(schedule_failure(
                context,
                StorageError::InvalidRecord,
                mutation,
                None,
            ))
        }),
        Err(error) => {
            let mutation = if mutating && matches!(error, StorageError::Database(_)) {
                LocalMutationState::Unknown
            } else {
                LocalMutationState::None
            };
            let current = match (&error, &context.schedule_id) {
                (StorageError::ScheduleChangeConflict, Some(id)) => store
                    .lock()
                    .await
                    .inspect_schedule::<SessionRef, EndpointRef>(id)
                    .await
                    .ok()
                    .map(|record| record.record.change_id),
                _ => None,
            };
            Err(ScheduleOperationFailure::Operation(schedule_failure(
                context, error, mutation, current,
            )))
        }
    }
}

/// The identities a schedule failure reports back, taken from the request that failed.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScheduleFailureContext {
    pub(crate) operation_id: Option<OperationId>,
    pub(crate) schedule_id: Option<ScheduleId>,
}

impl ScheduleFailureContext {
    pub(crate) fn new(operation_id: Option<OperationId>, schedule_id: Option<ScheduleId>) -> Self {
        Self {
            operation_id,
            schedule_id,
        }
    }
}

/// The failure for a request that does not have the published closed schedule shape.
pub(crate) fn invalid_schedule_request(
    context: ScheduleFailureContext,
) -> ScheduleOperationFailure {
    ScheduleOperationFailure::Operation(schedule_failure(
        context,
        StorageError::InvalidSchedule {
            field: "request",
            reason: "Use the published closed schedule request shape, UUIDv7 identities and explicit nullable timeout.",
        },
        LocalMutationState::None,
        None,
    ))
}

/// A schedule failure with the kind, stage, field and next action its storage error implies.
pub(crate) fn schedule_failure(
    context: ScheduleFailureContext,
    error: StorageError,
    mutation: LocalMutationState,
    current_change_id: Option<ChangeId>,
) -> ScheduleFailure {
    let (kind, stage, next_action, field, constraint) = match &error {
        StorageError::ScheduleImportExists => (
            ScheduleFailureKind::InvalidField, ScheduleFailureStage::Admission,
            ScheduleNextAction::CorrectRequest, Some("overwrite".into()),
            Some("An existing schedule UUID requires explicit --overwrite, even for identical content.".into()),
        ),
        StorageError::InstructionImportConflict { .. } => (
            ScheduleFailureKind::InstructionConflict, ScheduleFailureStage::Admission,
            ScheduleNextAction::CorrectRequest, Some("packageUtf8".into()),
            Some("Edit the shared instructions explicitly or import a deliberately new instruction identity; --overwrite applies only to the schedule.".into()),
        ),
        StorageError::InvalidPackage(error) => (
            ScheduleFailureKind::InvalidField, ScheduleFailureStage::Validation,
            ScheduleNextAction::CorrectRequest, Some("packageUtf8".into()), Some(error.to_string()),
        ),
        StorageError::InvalidSchedule { field, reason } => (
            ScheduleFailureKind::InvalidField,
            ScheduleFailureStage::Validation,
            ScheduleNextAction::CorrectRequest,
            Some((*field).to_owned()),
            Some((*reason).to_owned()),
        ),
        StorageError::InvalidTiming(error) => (
            ScheduleFailureKind::InvalidField,
            ScheduleFailureStage::Validation,
            ScheduleNextAction::CorrectRequest,
            Some("timing".into()),
            Some(error.to_string()),
        ),
        StorageError::ActivationUnavailable => (
            ScheduleFailureKind::UnsupportedCapability,
            ScheduleFailureStage::Admission,
            ScheduleNextAction::InspectEndpointCapabilities,
            None,
            None,
        ),
        StorageError::ScheduleChangeConflict => (
            ScheduleFailureKind::ChangeConflict,
            ScheduleFailureStage::Admission,
            ScheduleNextAction::InspectSchedule,
            None,
            None,
        ),
        StorageError::OperationConflict => (
            ScheduleFailureKind::OperationConflict,
            ScheduleFailureStage::Admission,
            ScheduleNextAction::InspectOperation,
            None,
            None,
        ),
        StorageError::ScheduleNotFound | StorageError::InstructionNotFound => (
            ScheduleFailureKind::ResourceNotFound,
            ScheduleFailureStage::Inspection,
            ScheduleNextAction::CorrectRequest,
            None,
            None,
        ),
        StorageError::Database(_) => (
            ScheduleFailureKind::AutomationUnavailable,
            ScheduleFailureStage::Storage,
            ScheduleNextAction::InspectOperation,
            None,
            None,
        ),
        _ => (
            ScheduleFailureKind::InvalidRecord,
            ScheduleFailureStage::Storage,
            ScheduleNextAction::InspectSchedule,
            None,
            None,
        ),
    };
    let message = if matches!(
        mutation,
        LocalMutationState::Unknown | LocalMutationState::Committed
    ) {
        "Schedule mutation may exist; inspect or replay the same operation ID before any new request.".into()
    } else {
        error.to_string()
    };
    ScheduleFailure {
        kind,
        stage,
        message,
        operation_id: context.operation_id,
        schedule_id: context.schedule_id,
        current_change_id,
        field,
        constraint,
        details: match error {
            StorageError::InstructionImportConflict {
                instruction_id,
                schedule_ids,
            } => collaboration_protocol::ScheduleFailureDetails::InstructionConflict {
                instruction_id,
                schedule_ids,
            },
            _ => collaboration_protocol::ScheduleFailureDetails::None,
        },
        effects: ScheduleEffects::Local { mutation },
        next_action,
    }
}

/// Why a schedule operation failed; every variant carries the typed `ScheduleFailure`.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ScheduleOperationFailure {
    /// Local administration or a package transfer was refused or could not complete.
    #[error("Schedule operation failed")]
    Operation(ScheduleFailure),
    /// The package, or the request that would import it, exceeds the response limit.
    #[error("Schedule package too large")]
    PackageTooLarge(ScheduleFailure),
    /// Preparation was refused or its route failed.
    #[error("Schedule preparation failed")]
    Preparation(ScheduleFailure),
    /// A retried preparation returns the failure the original attempt recorded.
    #[error("Retained preparation outcome")]
    RetainedPreparation(ScheduleFailure),
}

impl ScheduleOperationFailure {
    /// The typed failure this variant carries.
    #[must_use]
    pub const fn failure(&self) -> &ScheduleFailure {
        match self {
            Self::Operation(failure)
            | Self::PackageTooLarge(failure)
            | Self::Preparation(failure)
            | Self::RetainedPreparation(failure) => failure,
        }
    }
}

impl CollaborationRejection for ScheduleOperationFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self {
            Self::PackageTooLarge(_) => None,
            Self::Operation(failure)
            | Self::Preparation(failure)
            | Self::RetainedPreparation(failure) => failure.rejection_reason(),
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        PublishedRejection::typed(
            PublishedRejection::OPERATION_FAILED,
            self.to_string(),
            self.failure(),
        )
    }
}
