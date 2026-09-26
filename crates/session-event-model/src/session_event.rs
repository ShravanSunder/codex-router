use serde::{Deserialize, Serialize};

use crate::{CapabilityReport, PendingInteraction, SessionState};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalCause {
    OutputOverflow,
    Other(String),
}

/// Ended carries an agent-confirmed stop reason; lost carries no such claim.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TurnOutcome {
    Ended {
        stop_reason: StopReason,
        local_cause: Option<LocalCause>,
    },
    Lost {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SessionItemKind {
    UserMessage,
    AgentMessage,
    AgentThought,
    ToolCall {
        tool_kind: String,
        status: ToolCallStatus,
    },
    Plan,
    Usage,
    ModeChange,
    ConfigChange,
    SessionInfo,
    Notice,
    Unknown {
        source_kind: String,
    },
}

/// Text is retained for history and simple facades; structured payloads can
/// be added at this boundary when a consumer needs their exact shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionItem {
    pub item_id: String,
    pub kind: SessionItemKind,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum QuestionField {
    Text {
        field_id: String,
        required: bool,
    },
    Number {
        field_id: String,
        required: bool,
    },
    Boolean {
        field_id: String,
        required: bool,
    },
    SingleChoice {
        field_id: String,
        required: bool,
        options: Vec<String>,
    },
}

/// Events carry domain facts; the hub assigns sequence numbers and owns replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SessionEvent {
    TurnStarted {
        turn_id: String,
        input_id: String,
    },
    TurnEnded {
        turn_id: String,
        outcome: TurnOutcome,
    },
    ItemStarted {
        item: SessionItem,
    },
    ItemUpdated {
        item: SessionItem,
    },
    ItemCompleted {
        item_id: String,
    },
    InteractionRequested {
        interaction: PendingInteraction,
    },
    InteractionResolved {
        request_id: String,
    },
    StateChanged {
        state: SessionState,
    },
    CapabilitiesChanged {
        capabilities: CapabilityReport,
    },
}
