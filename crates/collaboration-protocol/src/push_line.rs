//! Stored push identities and the one-line Router delivery format.

use crate::{SessionDisplayName, SessionRef, UuidIdentity};
use agent_automation::{RunId, ScheduleId};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt};

pub const MAX_PUSH_LINE_BYTES: usize = 1024;
pub const MAX_MACHINE_LABEL_SCALARS: usize = 120;
const PREVIEW_MAX_SCALARS: usize = 100;
const MAX_SUBSCRIPTION_ROOTS: u8 = 20;
const MAX_HELD_SINCE_SCALARS: usize = 64;
const MAX_SUBSCRIPTION_SCOPE_SCALARS: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MachineLabel(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("machine label must contain non-whitespace text")]
pub struct MachineLabelError;

impl TryFrom<String> for MachineLabel {
    type Error = MachineLabelError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(MachineLabelError);
        }
        let mut characters = trimmed.chars();
        let bounded = if trimmed.chars().count() > MAX_MACHINE_LABEL_SCALARS {
            let mut prefix: String = characters
                .by_ref()
                .take(MAX_MACHINE_LABEL_SCALARS - 1)
                .collect();
            prefix.push('…');
            prefix
        } else {
            trimmed.to_owned()
        };
        Ok(Self(bounded))
    }
}

impl From<MachineLabel> for String {
    fn from(value: MachineLabel) -> Self {
        value.0
    }
}

impl MachineLabel {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl JsonSchema for MachineLabel {
    fn schema_name() -> Cow<'static, str> {
        "MachineLabel".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string","minLength":1,"maxLength":120})
    }
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MachineId(String);

impl TryFrom<String> for MachineId {
    type Error = PushIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        UuidIdentity::try_from(value.clone()).map_err(|_| PushIdError::MachineId)?;
        Ok(Self(value))
    }
}

impl From<UuidIdentity> for MachineId {
    fn from(value: UuidIdentity) -> Self {
        Self(String::from(value))
    }
}

impl From<MachineId> for String {
    fn from(value: MachineId) -> Self {
        value.0
    }
}

impl MachineId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PushId(String);

impl TryFrom<String> for PushId {
    type Error = PushIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        UuidIdentity::try_from(value.clone()).map_err(|_| PushIdError::PushId)?;
        let bytes = value.as_bytes();
        let version_is_seven = bytes.get(14) == Some(&b'7');
        let variant_is_rfc4122 = matches!(bytes.get(19), Some(b'8' | b'9' | b'a' | b'b'));
        if !version_is_seven || !variant_is_rfc4122 {
            return Err(PushIdError::PushId);
        }
        Ok(Self(value))
    }
}

impl From<PushId> for String {
    fn from(value: PushId) -> Self {
        value.0
    }
}

impl PushId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct RouterLink {
    machine_id: MachineId,
    push_id: PushId,
}

impl RouterLink {
    #[must_use]
    pub const fn new(machine_id: MachineId, push_id: PushId) -> Self {
        Self {
            machine_id,
            push_id,
        }
    }

    pub fn parse(value: &str) -> Result<Self, PushIdError> {
        let suffix = value.strip_prefix("router://").ok_or(PushIdError::Link)?;
        let (machine_id, push_id) = suffix.split_once("/push/").ok_or(PushIdError::Link)?;
        if push_id.contains('/') || push_id.contains('?') || push_id.contains('#') {
            return Err(PushIdError::Link);
        }
        Ok(Self {
            machine_id: MachineId::try_from(machine_id.to_owned())?,
            push_id: PushId::try_from(push_id.to_owned())?,
        })
    }

    #[must_use]
    pub const fn machine_id(&self) -> &MachineId {
        &self.machine_id
    }

    #[must_use]
    pub const fn push_id(&self) -> &PushId {
        &self.push_id
    }
}

impl fmt::Display for RouterLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "router://{}/push/{}",
            self.machine_id.as_str(),
            self.push_id.as_str()
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PushKind {
    DirectMessage,
    Wake,
    ScheduleRun,
    Approval,
    Question,
    SubscriptionActivity,
    SubscriptionExpiry,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PushOrigin {
    Session(SessionRef),
    OwnerUnverified,
    Router(PushKind),
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PushHeaderFacts {
    DirectMessage {
        sender_display_name: Option<SessionDisplayName>,
    },
    Wake,
    ScheduleRun {
        schedule_id: ScheduleId,
        run_id: RunId,
    },
    Approval {
        requester: SessionRef,
        requester_display_name: Option<SessionDisplayName>,
    },
    Question {
        requester: SessionRef,
        requester_display_name: Option<SessionDisplayName>,
    },
    SubscriptionActivity {
        root_count: u8,
        message_count: u64,
        held_since: Option<String>,
        thread_resolved: bool,
    },
    SubscriptionExpiry {
        scope: String,
    },
}

impl PushHeaderFacts {
    #[must_use]
    pub const fn kind(&self) -> PushKind {
        match self {
            Self::DirectMessage { .. } => PushKind::DirectMessage,
            Self::Wake => PushKind::Wake,
            Self::ScheduleRun { .. } => PushKind::ScheduleRun,
            Self::Approval { .. } => PushKind::Approval,
            Self::Question { .. } => PushKind::Question,
            Self::SubscriptionActivity { .. } => PushKind::SubscriptionActivity,
            Self::SubscriptionExpiry { .. } => PushKind::SubscriptionExpiry,
        }
    }
}

pub struct PushLineInput {
    pub link: RouterLink,
    pub machine_label: MachineLabel,
    pub origin: PushOrigin,
    pub header_facts: PushHeaderFacts,
    pub body: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedPushLineHeader {
    pub kind: PushKind,
    pub title: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PushIdError {
    #[error("invalid machine id")]
    MachineId,
    #[error("push id must be a canonical UUIDv7")]
    PushId,
    #[error("invalid Router push link")]
    Link,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PushLineError {
    #[error("push origin and header kind do not agree")]
    OriginKindMismatch,
    #[error("push body is required for this kind")]
    MissingBody,
    #[error("subscription notices cannot carry a preview body")]
    UnexpectedBody,
    #[error("invalid push header facts")]
    InvalidHeaderFacts,
    #[error("the fixed push header and link exceed the line byte budget")]
    FixedLineExceedsBudget,
}

#[path = "push_line_render.rs"]
mod rendering;
pub use rendering::{escape_push_line_field, parse_push_line_header, render_push_line};
