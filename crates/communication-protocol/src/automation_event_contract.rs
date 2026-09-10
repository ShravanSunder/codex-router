//! Structured domain events are observations, never executable prompts or native transcripts.
use crate::{
    AttemptInspection, ObservationTimestamp, PageLimit, PositiveSeconds, ScheduleDefinition,
    SummaryInspection, WakeDefinition,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutomationEventSubjectKind {
    Instruction,
    Schedule,
    Wake,
    Run,
    Delivery,
    Configuration,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationEventSubject {
    pub kind: AutomationEventSubjectKind,
    pub id: String,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AutomationEventDetails {
    ScheduleEdit {
        #[serde(deserialize_with = "Option::deserialize")]
        before: Option<Box<ScheduleDefinition>>,
        after: Box<ScheduleDefinition>,
    },
    WakeEdit {
        #[serde(deserialize_with = "Option::deserialize")]
        before: Option<Box<WakeDefinition>>,
        after: Box<WakeDefinition>,
    },
    DeliveryAttempt {
        attempt: Box<AttemptInspection>,
    },
    SummaryAttempt {
        attempt: Box<SummaryInspection>,
    },
    StateChange {
        #[serde(deserialize_with = "Option::deserialize")]
        before: Option<String>,
        after: String,
    },
    ConfigurationChange {
        execution_timeout_seconds: PositiveSeconds,
        summary_timeout_seconds: PositiveSeconds,
    },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationEvent {
    pub event_id: agent_automation::EventId,
    pub cursor: String,
    pub recorded_at: ObservationTimestamp,
    pub subject: AutomationEventSubject,
    pub change: String,
    pub description: String,
    pub details: AutomationEventDetails,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationEventsRequest {
    #[serde(deserialize_with = "Option::deserialize")]
    pub after: Option<String>,
    pub limit: PageLimit,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationEventsPage {
    pub records: Vec<AutomationEvent>,
    pub next_cursor: String,
    pub earliest_retained_cursor: String,
}
