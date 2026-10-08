//! Each family's typed failure maps to a spec 1 §5 rejection reason only where it is that same
//! reason, and its wire payload keeps today's `kind`. The oracle is the spec 1 §5 table and the
//! published `-32050` payload kinds, never the mapping code itself.
use super::automation_operations::{
    InstructionContext, InstructionFailureReason, instruction_failure, run_failure_context,
};
use super::schedule_operations::{ScheduleFailureContext, schedule_failure};
use super::wake_operations::{WakeFailureContext, wake_failure};
use super::*;
use CollaborationRejectionReason as Reason;
use automation_storage::StorageError;
use collaboration_protocol::{
    AutomationInspectionFailureKind, ConfigurationFailure, ConfigurationFailureKind,
    ConfigurationFileState, ConfigurationNextAction, ConversationOperationFailure,
    ConversationOperationFailureKind, ConversationOperationFailureStage, InstructionFailureKind,
    InstructionStage, LocalMutationState, NonEmptyText, ProviderInspectFailure,
    ProviderInspectFailureKind, ProviderOperationEffect, ProviderSettingsFailure,
    ProviderSettingsFailureKind, RunFailureKind, ScheduleFailure, WakeFailureReason,
};
use message_board::{
    BoardError, BoardErrorDetails, BoardFailureKind, BoardFailureStage, BoardNextAction,
};
use serde_json::{Value, json};

fn wire_kind(failure: &impl serde::Serialize) -> String {
    let payload = serde_json::to_value(failure).expect("failure payload encodes");
    payload
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .expect("failure payload names its kind")
}

#[test]
fn every_reason_carries_its_spec_name() {
    // Arrange: the spec 1 §5 table.
    let table = [
        (Reason::Overloaded, "overloaded"),
        (Reason::Archived, "archived"),
        (Reason::InvalidShape, "invalidShape"),
        (Reason::CursorInvalid, "cursorInvalid"),
        (Reason::StaleRevision, "staleRevision"),
        (Reason::ConflictingRequest, "conflictingRequest"),
    ];

    // Act & assert.
    for (reason, spec_name) in table {
        assert_eq!(reason.spec_name(), spec_name);
    }
}

#[test]
fn board_failures_map_by_kind() {
    // Arrange: today's board kinds and the spec reason each one is, if any.
    let table = [
        (
            BoardFailureKind::Overloaded,
            "overloaded",
            Some(Reason::Overloaded),
        ),
        (
            BoardFailureKind::ArchivedBoard,
            "archivedBoard",
            Some(Reason::Archived),
        ),
        (
            BoardFailureKind::InvalidField,
            "invalidField",
            Some(Reason::InvalidShape),
        ),
        (
            BoardFailureKind::InvalidIdentity,
            "invalidIdentity",
            Some(Reason::InvalidShape),
        ),
        (
            BoardFailureKind::InvalidCursor,
            "invalidCursor",
            Some(Reason::CursorInvalid),
        ),
        (BoardFailureKind::ThreadResolved, "threadResolved", None),
        (BoardFailureKind::OutcomeUnknown, "outcomeUnknown", None),
        (
            BoardFailureKind::ResourceAlreadyExists,
            "resourceAlreadyExists",
            None,
        ),
        (
            BoardFailureKind::StaleOrchestrator,
            "staleOrchestrator",
            None,
        ),
    ];
    for (kind, wire, reason) in table {
        let failure = BoardError {
            kind,
            stage: BoardFailureStage::Validation,
            message: "failure".into(),
            next_action: BoardNextAction::CorrectRequest,
            details: BoardErrorDetails::None,
        };

        // Act & assert.
        assert_eq!(failure.rejection_reason(), reason, "{wire}");
        assert_eq!(wire_kind(&failure), wire);
    }
}

#[test]
fn wake_failures_map_by_reason() {
    // Arrange.
    let table = [
        (
            WakeFailureReason::Overloaded,
            "overloaded",
            Some(Reason::Overloaded),
        ),
        (
            WakeFailureReason::InvalidField {
                field: "timing".into(),
                constraint: "bounded".into(),
            },
            "invalidField",
            Some(Reason::InvalidShape),
        ),
        (
            WakeFailureReason::OperationConflict,
            "operationConflict",
            Some(Reason::ConflictingRequest),
        ),
        (
            WakeFailureReason::LifecycleConflict,
            "lifecycleConflict",
            None,
        ),
        (
            WakeFailureReason::ResourceNotFound,
            "resourceNotFound",
            None,
        ),
        (
            WakeFailureReason::AutomationUnavailable,
            "automationUnavailable",
            None,
        ),
    ];
    for (reason_kind, wire, reason) in table {
        // Act.
        let failure = wake_failure(
            WakeFailureContext::default(),
            reason_kind,
            LocalMutationState::None,
        );

        // Assert.
        assert_eq!(failure.rejection_reason(), reason, "{wire}");
        assert_eq!(wire_kind(&failure), wire);
    }
    let wait = WakeWaitFailure::unavailable(agent_automation::WakeupId::generate());
    assert_eq!(wait.rejection_reason(), None);
}

#[test]
fn schedule_failures_map_by_storage_outcome() {
    // Arrange.
    let table = [
        (
            StorageError::ScheduleChangeConflict,
            "changeConflict",
            Some(Reason::StaleRevision),
        ),
        (
            StorageError::OperationConflict,
            "operationConflict",
            Some(Reason::ConflictingRequest),
        ),
        (
            StorageError::InvalidSchedule {
                field: "request",
                reason: "closed shape",
            },
            "invalidField",
            Some(Reason::InvalidShape),
        ),
        (StorageError::ScheduleNotFound, "resourceNotFound", None),
        (
            StorageError::ActivationUnavailable,
            "unsupportedCapability",
            None,
        ),
    ];
    for (error, wire, reason) in table {
        // Act.
        let failure = schedule_failure(
            ScheduleFailureContext::default(),
            error,
            LocalMutationState::None,
            None,
        );

        // Assert.
        assert_eq!(failure.rejection_reason(), reason, "{wire}");
        assert_eq!(wire_kind(&failure), wire);
        let preparation = ScheduleOperationFailure::Preparation(failure);
        assert_eq!(preparation.rejection_reason(), reason, "{wire}");
    }
}

#[test]
fn an_oversized_schedule_package_is_not_a_request_shape_rejection() {
    // Arrange: the export failure keeps its published invalidField payload.
    let failure = ScheduleOperationFailure::PackageTooLarge(ScheduleFailure::package_frame_limit(
        None, 2_000_000,
    ));

    // Act & assert.
    assert_eq!(failure.rejection_reason(), None);
    assert_eq!(wire_kind(failure.failure()), "invalidField");
}

#[test]
fn instruction_failures_map_by_kind() {
    // Arrange.
    let table = [
        (
            InstructionFailureKind::Overloaded,
            "overloaded",
            Some(Reason::Overloaded),
        ),
        (
            InstructionFailureKind::InvalidField,
            "invalidField",
            Some(Reason::InvalidShape),
        ),
        (
            InstructionFailureKind::OperationConflict,
            "operationConflict",
            Some(Reason::ConflictingRequest),
        ),
        (
            InstructionFailureKind::RevisionConflict,
            "revisionConflict",
            Some(Reason::StaleRevision),
        ),
        (
            InstructionFailureKind::ResourceNotFound,
            "resourceNotFound",
            None,
        ),
        (InstructionFailureKind::InvalidRecord, "invalidRecord", None),
    ];
    for (kind, wire, reason) in table {
        // Act.
        let failure = instruction_failure(
            InstructionContext {
                operation_id: None,
                instruction_id: None,
                mutation: true,
                current_revision_id: None,
            },
            InstructionFailureReason {
                kind,
                stage: InstructionStage::Storage,
                message: "failure",
                mutation: LocalMutationState::None,
            },
        );

        // Assert.
        assert_eq!(failure.rejection_reason(), reason, "{wire}");
        assert_eq!(wire_kind(&failure), wire);
    }
}

#[test]
fn run_configuration_and_inspection_failures_map_by_kind() {
    // Arrange, act and assert: Runs.
    let runs = [
        (
            RunFailureKind::Overloaded,
            "overloaded",
            Some(Reason::Overloaded),
        ),
        (
            RunFailureKind::InvalidField,
            "invalidField",
            Some(Reason::InvalidShape),
        ),
        (
            RunFailureKind::OperationConflict,
            "operationConflict",
            Some(Reason::ConflictingRequest),
        ),
        (
            RunFailureKind::RecoveryNotAllowed,
            "recoveryNotAllowed",
            None,
        ),
    ];
    for (kind, wire, reason) in runs {
        let mut failure = run_failure_context(None, None);
        failure.kind = kind;
        assert_eq!(failure.rejection_reason(), reason, "{wire}");
        assert_eq!(wire_kind(&failure), wire);
    }

    // Configuration.
    let configurations = [
        (
            ConfigurationFailureKind::InvalidField,
            "invalidField",
            Some(Reason::InvalidShape),
        ),
        (
            ConfigurationFailureKind::OperationConflict,
            "operationConflict",
            Some(Reason::ConflictingRequest),
        ),
        (
            ConfigurationFailureKind::AutomationUnavailable,
            "automationUnavailable",
            None,
        ),
        (
            ConfigurationFailureKind::OutcomeUnknown,
            "outcomeUnknown",
            None,
        ),
    ];
    for (kind, wire, reason) in configurations {
        let failure = ConfigurationFailure {
            kind,
            message: "failure".into(),
            operation_id: None,
            file_state: ConfigurationFileState::NotReplaced,
            next_action: ConfigurationNextAction::RetryLater,
        };
        assert_eq!(failure.rejection_reason(), reason, "{wire}");
        assert_eq!(wire_kind(&failure), wire);
    }

    // Automation inspection.
    let inspections = [
        (
            AutomationInspectionFailureKind::Overloaded,
            "overloaded",
            Some(Reason::Overloaded),
        ),
        (
            AutomationInspectionFailureKind::InvalidField,
            "invalidField",
            Some(Reason::InvalidShape),
        ),
        (
            AutomationInspectionFailureKind::HistoryExpired,
            "historyExpired",
            None,
        ),
        (
            AutomationInspectionFailureKind::ResourceNotFound,
            "resourceNotFound",
            None,
        ),
    ];
    for (kind, wire, reason) in inspections {
        let mut failure = crate::automation_inspection_failure::invalid("cursor", "stale");
        failure.kind = kind;
        assert_eq!(failure.rejection_reason(), reason, "{wire}");
        assert_eq!(wire_kind(&failure), wire);
    }
}

#[test]
fn conversation_failures_map_only_overload() {
    // Arrange.
    let operation = |kind| {
        ConversationFailure::Operation(ConversationOperationFailure {
            kind,
            stage: ConversationOperationFailureStage::Binding,
            effect: ProviderOperationEffect::None,
            message: NonEmptyText::try_from("failure".to_owned()).expect("message text"),
            operation_id: None,
            invalid_setting: None,
            provider_code: None,
            target: None,
            endpoint: None,
            availability: None,
        })
    };
    let settings = |kind| ProviderSettingsFailure {
        kind,
        stage: None,
        target: None,
        message: "failure".into(),
        setting: None,
        value: None,
        advertised: Vec::new(),
    };
    let inspect = |kind| ProviderInspectFailure {
        kind,
        stage: None,
        target: None,
        message: "failure".into(),
    };

    // Act & assert.
    assert_eq!(
        operation(ConversationOperationFailureKind::Overloaded).rejection_reason(),
        Some(Reason::Overloaded)
    );
    assert_eq!(
        operation(ConversationOperationFailureKind::StaleGeneration).rejection_reason(),
        None
    );
    assert_eq!(
        operation(ConversationOperationFailureKind::InvalidRequest).rejection_reason(),
        None
    );
    assert_eq!(
        ConversationFailure::InvalidStoredOperation.rejection_reason(),
        None
    );
    assert_eq!(
        settings(ProviderSettingsFailureKind::Overloaded).rejection_reason(),
        Some(Reason::Overloaded)
    );
    assert_eq!(
        settings(ProviderSettingsFailureKind::Busy).rejection_reason(),
        None
    );
    assert_eq!(
        inspect(ProviderInspectFailureKind::Overloaded).rejection_reason(),
        Some(Reason::Overloaded)
    );
    assert_eq!(
        inspect(ProviderInspectFailureKind::NotFound).rejection_reason(),
        None
    );
}

#[test]
fn message_interaction_and_observation_payloads_keep_their_kinds() {
    // Arrange, act and assert: messages.
    let invalid = MessageFailure {
        kind: MessageFailureKind::InvalidField,
        stage: MessageFailureStage::Discovery,
        message: "Invalid push id or Router link".into(),
    };
    assert_eq!(invalid.rejection_reason(), Some(Reason::InvalidShape));
    assert_eq!(
        serde_json::to_value(&invalid).expect("message failure encodes"),
        json!({"kind":"invalidField","stage":"discovery","message":"Invalid push id or Router link"})
    );
    let foreign = MessageFailure {
        kind: MessageFailureKind::ForeignMachine,
        stage: MessageFailureStage::Inspect,
        message: "lives elsewhere".into(),
    };
    assert_eq!(foreign.rejection_reason(), None);
    assert_eq!(wire_kind(&foreign), "foreignMachine");

    // Interactions are family-specific.
    let answer = InteractionFailure::QuestionRejected(QuestionRejection::InvalidAnswer {
        field_id: "choice".into(),
    });
    assert_eq!(answer.rejection_reason(), None);
    assert_eq!(
        answer.payload(),
        json!({"kind":"invalidAnswer","stage":"inspect","message":"Question response rejected","fieldId":"choice"})
    );
    assert_eq!(
        InteractionFailure::BrokerUnavailable.payload(),
        json!({"kind":"unavailable","stage":"inspect","message":"Approval service unavailable"})
    );

    // Observation.
    assert_eq!(
        ObservationFailure::InvalidField.rejection_reason(),
        Some(Reason::InvalidShape)
    );
    assert_eq!(ObservationFailure::NotFound.rejection_reason(), None);
    assert_eq!(
        serde_json::to_value(ObservationFailure::NotFound).expect("observation failure encodes"),
        json!({"kind":"notFound","stage":"inspect","message":"Provider Session was not found"})
    );
}

#[test]
fn a_result_too_large_for_its_response_is_not_the_concurrency_limit() {
    // Arrange: both keep the published "overloaded" payload kind.
    let native = NativeSessionFailure::refused(
        NativeSessionFailureKind::ResponseTooLarge,
        NativeSessionStage::Inspect,
    );
    let inventory =
        ProviderInventoryFailure::Unavailable(ProviderInventoryFailureKind::ResponseTooLarge);

    // Act & assert.
    assert_eq!(native.rejection_reason(), None);
    assert_eq!(wire_kind(&native), "overloaded");
    assert_eq!(inventory.rejection_reason(), None);
    assert_eq!(wire_kind(&inventory), "overloaded");
    assert_eq!(
        NativeSessionFailure::InvalidRequest(INVALID_NATIVE_PARAMETERS).rejection_reason(),
        Some(Reason::InvalidShape)
    );
    assert_eq!(
        ProviderInventoryFailure::InvalidRequest("Provider session page size must be 1..100")
            .rejection_reason(),
        Some(Reason::InvalidShape)
    );
}

#[test]
fn a_full_address_snapshot_cache_is_overload() {
    // Arrange.
    let full = JournalFailure::Unavailable(JournalUnavailableKind::Overloaded);
    let unknown_endpoint = JournalFailure::Unavailable(JournalUnavailableKind::EndpointNotFound);

    // Act & assert.
    assert_eq!(full.rejection_reason(), Some(Reason::Overloaded));
    assert_eq!(wire_kind(&full), "overloaded");
    assert_eq!(unknown_endpoint.rejection_reason(), None);
    assert_eq!(JournalFailure::SnapshotExpired.rejection_reason(), None);
    assert_eq!(
        JournalFailure::InvalidRequest.rejection_reason(),
        Some(Reason::InvalidShape)
    );
}
