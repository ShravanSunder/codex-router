use crate::{ActivitySequence, Identity};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

pub const MAX_PARTICIPANT_NOTE_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ParticipantRole {
    Orchestrator,
    Implementer,
    Advisor,
    Reviewer,
    Participant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ParticipantClosedReason {
    Left,
    Replaced,
    Resolved,
}

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("participant note must contain 1 to 16384 trimmed UTF-8 bytes without NUL")]
pub struct InvalidParticipantNote;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ParticipantNote(String);

impl TryFrom<String> for ParticipantNote {
    type Error = InvalidParticipantNote;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.len() > MAX_PARTICIPANT_NOTE_BYTES
            || trimmed.contains('\0')
        {
            return Err(InvalidParticipantNote);
        }
        Ok(Self(trimmed.to_owned()))
    }
}

impl From<ParticipantNote> for String {
    fn from(note: ParticipantNote) -> Self {
        note.0
    }
}

impl ParticipantNote {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl JsonSchema for ParticipantNote {
    fn schema_name() -> Cow<'static, str> {
        "ParticipantNote".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({"type":"string","minLength":1,"maxLength":16384})
    }
}

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("participant open and closed fields do not form a valid Participant state")]
pub struct InvalidParticipantState;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Participant {
    pub identity: Identity,
    pub role: ParticipantRole,
    pub note: Option<ParticipantNote>,
    pub joined_at_activity: ActivitySequence,
    pub last_seen_activity: ActivitySequence,
    pub closed_at_activity: Option<ActivitySequence>,
    pub closed_reason: Option<ParticipantClosedReason>,
    pub replaced_by: Option<Identity>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ParticipantWire {
    identity: Identity,
    role: ParticipantRole,
    note: Option<ParticipantNote>,
    joined_at_activity: ActivitySequence,
    last_seen_activity: ActivitySequence,
    closed_at_activity: Option<ActivitySequence>,
    closed_reason: Option<ParticipantClosedReason>,
    replaced_by: Option<Identity>,
}

impl<'de> Deserialize<'de> for Participant {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ParticipantWire::deserialize(deserializer)?;
        Self::new(
            wire.identity,
            wire.role,
            wire.note,
            wire.joined_at_activity,
            wire.last_seen_activity,
            wire.closed_at_activity,
            wire.closed_reason,
            wire.replaced_by,
        )
        .map_err(serde::de::Error::custom)
    }
}

impl Participant {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Identity,
        role: ParticipantRole,
        note: Option<ParticipantNote>,
        joined_at_activity: ActivitySequence,
        last_seen_activity: ActivitySequence,
        closed_at_activity: Option<ActivitySequence>,
        closed_reason: Option<ParticipantClosedReason>,
        replaced_by: Option<Identity>,
    ) -> Result<Self, InvalidParticipantState> {
        if last_seen_activity < joined_at_activity {
            return Err(InvalidParticipantState);
        }
        match (closed_at_activity, closed_reason, replaced_by.as_ref()) {
            (None, None, None) => {}
            (Some(closed), Some(ParticipantClosedReason::Left), None)
            | (Some(closed), Some(ParticipantClosedReason::Resolved), None)
                if closed == last_seen_activity => {}
            (Some(closed), Some(ParticipantClosedReason::Replaced), Some(replacement))
                if matches!(
                    role,
                    ParticipantRole::Orchestrator | ParticipantRole::Implementer
                ) && closed == last_seen_activity
                    && *replacement != identity => {}
            _ => return Err(InvalidParticipantState),
        }
        Ok(Self {
            identity,
            role,
            note,
            joined_at_activity,
            last_seen_activity,
            closed_at_activity,
            closed_reason,
            replaced_by,
        })
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.closed_at_activity.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestratorHolder {
    pub identity: Identity,
    pub last_seen_activity: ActivitySequence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImplementerHolder {
    pub identity: Identity,
    pub last_seen_activity: ActivitySequence,
}
