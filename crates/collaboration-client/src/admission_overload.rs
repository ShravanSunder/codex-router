//! The API's admission overload, answered in each family's own failure.
//!
//! A listener at its request limit answers a tool call with one generic payload,
//! `{kind: "overloaded", stage: "admission", effect: "none", message}`: the call did not
//! run, and the same operation identity may be retried. The client recognises it before any
//! family decode ([`ClientError::Overloaded`]), and each family client reports it as its own
//! typed `overloaded` failure with the next action "retry later", as the families always
//! have.
use crate::ClientError;
use collaboration_protocol::{
    AutomationInspectionFailure, AutomationInspectionFailureKind, AutomationInspectionNextAction,
    AutomationInspectionStage, ConfigurationFailure, ConfigurationFailureKind,
    ConfigurationFileState, ConfigurationNextAction, ConversationOperationFailure,
    ConversationOperationFailureKind, ConversationOperationFailureStage, InstructionFailure,
    InstructionFailureKind, InstructionNextAction, InstructionStage, LocalMutationEvidence,
    LocalMutationState, NonEmptyText, ProviderOperationEffect, RunFailure, RunFailureKind,
    RunFailureStage, RunNextAction, ScheduleEffects, ScheduleFailure, ScheduleFailureDetails,
    ScheduleFailureKind, ScheduleFailureStage, ScheduleNextAction, WakeFailure, WakeFailureReason,
    WakeFailureStage, WakeNextAction,
};
use message_board::{
    BoardError, BoardErrorDetails, BoardFailureKind, BoardFailureStage, BoardNextAction,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The overload a tool failure carries, if it is one.
pub(crate) fn admission_overload(failure: &Value) -> Option<ClientError> {
    let text = |field: &str| failure.get(field).and_then(Value::as_str);
    (text("kind") == Some("overloaded") && text("effect") == Some("none")).then(|| {
        ClientError::Overloaded {
            message: text("message")
                .filter(|message| !message.is_empty())
                .unwrap_or(DEFAULT_MESSAGE)
                .to_owned(),
        }
    })
}

const DEFAULT_MESSAGE: &str =
    "Request capacity exceeded; this request was not run. Retry when capacity is available.";

/// The identities a request carried, kept before it is sent so a shed answer can name them.
pub(crate) struct ShedRequest {
    identities: serde_json::Map<String, Value>,
}

impl ShedRequest {
    pub(crate) fn of(params: &Value) -> Self {
        let identities = [
            "operationId",
            "wakeupId",
            "scheduleId",
            "runId",
            "instructionId",
        ]
        .into_iter()
        .filter_map(|field| Some((field.to_owned(), params.get(field)?.clone())))
        .collect();
        Self { identities }
    }

    fn identity<TIdentity: DeserializeOwned>(&self, field: &str) -> Option<TIdentity> {
        self.identities
            .get(field)
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
    }
}

fn no_local_mutation() -> LocalMutationEvidence {
    LocalMutationEvidence::Local {
        mutation: LocalMutationState::None,
    }
}

pub(crate) fn board(message: String) -> BoardError {
    BoardError {
        kind: BoardFailureKind::Overloaded,
        stage: BoardFailureStage::Admission,
        message,
        next_action: BoardNextAction::RetryLater,
        details: BoardErrorDetails::None,
    }
}

pub(crate) fn wake(message: String, shed: &ShedRequest) -> WakeFailure {
    WakeFailure {
        reason: WakeFailureReason::Overloaded,
        stage: WakeFailureStage::Admission,
        message,
        operation_id: shed.identity("operationId"),
        wakeup_id: shed.identity("wakeupId"),
        effects: no_local_mutation(),
        next_action: WakeNextAction::RetryLater,
    }
}

pub(crate) fn schedule(message: String, shed: &ShedRequest) -> ScheduleFailure {
    ScheduleFailure {
        kind: ScheduleFailureKind::Overloaded,
        stage: ScheduleFailureStage::Admission,
        message,
        operation_id: shed.identity("operationId"),
        schedule_id: shed.identity("scheduleId"),
        current_change_id: None,
        field: None,
        constraint: None,
        details: ScheduleFailureDetails::None,
        effects: ScheduleEffects::Local {
            mutation: LocalMutationState::None,
        },
        next_action: ScheduleNextAction::RetryLater,
    }
}

pub(crate) fn run(message: String, shed: &ShedRequest) -> RunFailure {
    RunFailure {
        kind: RunFailureKind::Overloaded,
        stage: RunFailureStage::Admission,
        message,
        operation_id: shed.identity("operationId"),
        run_id: shed.identity("runId"),
        effects: no_local_mutation(),
        next_action: RunNextAction::RetryLater,
    }
}

pub(crate) fn instruction(message: String, shed: &ShedRequest) -> InstructionFailure {
    InstructionFailure {
        kind: InstructionFailureKind::Overloaded,
        stage: InstructionStage::Admission,
        message,
        operation_id: shed.identity("operationId"),
        instruction_id: shed.identity("instructionId"),
        current_revision_id: None,
        effects: no_local_mutation(),
        next_action: InstructionNextAction::RetryLater,
    }
}

pub(crate) fn configuration(message: String, shed: &ShedRequest) -> ConfigurationFailure {
    ConfigurationFailure {
        kind: ConfigurationFailureKind::AutomationUnavailable,
        message,
        operation_id: shed.identity("operationId"),
        file_state: ConfigurationFileState::NotReplaced,
        next_action: ConfigurationNextAction::RetryLater,
    }
}

pub(crate) fn inspection(message: String) -> AutomationInspectionFailure {
    AutomationInspectionFailure {
        kind: AutomationInspectionFailureKind::Overloaded,
        stage: AutomationInspectionStage::Inspection,
        message,
        resource_id: None,
        field: None,
        constraint: None,
        earliest_retained_cursor: None,
        effects: no_local_mutation(),
        next_action: AutomationInspectionNextAction::RetryLater,
    }
}

/// The conversation family's overload, for the CLI's typed conversation report.
#[must_use]
pub fn conversation(message: &str) -> Option<ConversationOperationFailure> {
    Some(ConversationOperationFailure {
        kind: ConversationOperationFailureKind::Overloaded,
        stage: ConversationOperationFailureStage::Admission,
        effect: ProviderOperationEffect::None,
        message: NonEmptyText::try_from(message.to_owned()).ok()?,
        operation_id: None,
        invalid_setting: None,
        provider_code: None,
        target: None,
        endpoint: None,
        availability: None,
    })
}
