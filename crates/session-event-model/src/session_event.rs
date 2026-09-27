use serde::{Deserialize, Serialize};

use crate::{CapabilityReport, PendingInteraction, SessionSettings, SessionState};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TurnLostReason {
    ProviderRetired,
    ProviderTurnFailed,
    EndNotObservable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum InteractionCancelReason {
    TurnCancelled,
    ProviderRetired,
    ApproverUnreachable,
    HostRestarted,
    EventPublicationFailed,
    AgentCancelled,
    TimedOut,
    Other(String),
}

impl InteractionCancelReason {
    pub fn as_str(&self) -> &str {
        match self {
            Self::TurnCancelled => "turnCancelled",
            Self::ProviderRetired => "providerRetired",
            Self::ApproverUnreachable => "approverUnreachable",
            Self::HostRestarted => "hostRestarted",
            Self::EventPublicationFailed => "eventPublicationFailed",
            Self::AgentCancelled => "agentCancelled",
            Self::TimedOut => "timedOut",
            Self::Other(reason) => reason,
        }
    }
}

impl From<String> for InteractionCancelReason {
    fn from(reason: String) -> Self {
        match reason.as_str() {
            "turnCancelled" => Self::TurnCancelled,
            "providerRetired" => Self::ProviderRetired,
            "approverUnreachable" => Self::ApproverUnreachable,
            "hostRestarted" => Self::HostRestarted,
            "eventPublicationFailed" => Self::EventPublicationFailed,
            "agentCancelled" => Self::AgentCancelled,
            "timedOut" => Self::TimedOut,
            _ => Self::Other(reason),
        }
    }
}

impl From<InteractionCancelReason> for String {
    fn from(reason: InteractionCancelReason) -> Self {
        reason.as_str().to_owned()
    }
}

impl schemars::JsonSchema for InteractionCancelReason {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "InteractionCancelReason".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ProviderSettingFailureReason {
    InvalidSetting,
    OutcomeUnknown,
    Other(String),
}

impl ProviderSettingFailureReason {
    pub fn as_str(&self) -> &str {
        match self {
            Self::InvalidSetting => "invalidSetting",
            Self::OutcomeUnknown => "outcomeUnknown",
            Self::Other(reason) => reason,
        }
    }
}

impl From<String> for ProviderSettingFailureReason {
    fn from(reason: String) -> Self {
        match reason.as_str() {
            "invalidSetting" => Self::InvalidSetting,
            "outcomeUnknown" => Self::OutcomeUnknown,
            _ => Self::Other(reason),
        }
    }
}

impl From<ProviderSettingFailureReason> for String {
    fn from(reason: ProviderSettingFailureReason) -> Self {
        reason.as_str().to_owned()
    }
}

impl schemars::JsonSchema for ProviderSettingFailureReason {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ProviderSettingFailureReason".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
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
        reason: TurnLostReason,
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

/// Events carry domain facts; the hub assigns sequence numbers and owns replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SessionEvent {
    TurnStarted {
        turn_id: String,
        input_id: crate::InputId,
    },
    InputAccepted {
        input_id: crate::InputId,
        turn_id: String,
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
    SettingsChanged {
        settings: SessionSettings,
    },
    /// Control event on an obsolete subscription when history replay resets.
    ResyncRequired {
        replay_epoch: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::{InteractionCancelReason, ProviderSettingFailureReason, TurnLostReason};

    #[test]
    fn typed_reasons_keep_existing_wire_codes() {
        assert_eq!(
            serde_json::to_string(&TurnLostReason::EndNotObservable).unwrap(),
            "\"endNotObservable\""
        );
        assert_eq!(
            serde_json::to_string(&InteractionCancelReason::TimedOut).unwrap(),
            "\"timedOut\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderSettingFailureReason::OutcomeUnknown).unwrap(),
            "\"outcomeUnknown\""
        );
        assert_eq!(
            serde_json::from_str::<InteractionCancelReason>("\"agent withdrew\"").unwrap(),
            InteractionCancelReason::Other("agent withdrew".to_owned())
        );
        assert_eq!(
            serde_json::from_str::<ProviderSettingFailureReason>("\"agent rejected\"").unwrap(),
            ProviderSettingFailureReason::Other("agent rejected".to_owned())
        );
    }
}
