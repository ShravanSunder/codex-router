//! Validated record shapes shared by Router storage and push-reading surfaces.
use crate::{
    DeliveryOutcome, DeliveryReceipt, MessageText, PushHeaderFacts, PushId, PushKind, PushOrigin,
    SessionRef,
};
use chrono::{DateTime, Utc};
use message_board::{ActivitySequence, MessageId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_DIRECT_MESSAGE_BODY_BYTES: usize = 65_536;
pub const MAX_PUSH_ACTIVITY_RANGES: usize = 20;

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushActivityRange {
    pub root_message_id: MessageId,
    pub from_activity_sequence: ActivitySequence,
    pub through_activity_sequence: ActivitySequence,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushActivitySnapshot {
    pub ranges: Vec<PushActivityRange>,
    pub held: bool,
    pub draining: bool,
}

impl PushActivitySnapshot {
    fn is_valid(&self) -> bool {
        !self.ranges.is_empty()
            && self.ranges.len() <= MAX_PUSH_ACTIVITY_RANGES
            && self
                .ranges
                .iter()
                .all(|range| range.from_activity_sequence <= range.through_activity_sequence)
            && self
                .ranges
                .iter()
                .map(|range| &range.root_message_id)
                .collect::<HashSet<_>>()
                .len()
                == self.ranges.len()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushRecordDraft {
    pub push_id: PushId,
    pub kind: PushKind,
    pub origin: PushOrigin,
    pub origin_router_ref: Option<String>,
    pub target: SessionRef,
    pub reply_to_push_id: Option<PushId>,
    pub header_facts: PushHeaderFacts,
    pub body: Option<String>,
    pub activity: Option<PushActivitySnapshot>,
    pub created_at: DateTime<Utc>,
}

impl PushRecordDraft {
    pub fn into_pending(self) -> Result<PushRecord, PushRecordValidationError> {
        if self.kind != self.header_facts.kind() {
            return Err(PushRecordValidationError::HeaderKindMismatch);
        }
        match (&self.kind, &self.origin) {
            (PushKind::DirectMessage, PushOrigin::Session(_))
            | (PushKind::DirectMessage, PushOrigin::OwnerUnverified) => {
                if self.origin_router_ref.is_some() {
                    return Err(PushRecordValidationError::InvalidOriginReference);
                }
            }
            (kind, PushOrigin::Router(origin_kind)) if kind == origin_kind => {}
            _ => return Err(PushRecordValidationError::OriginKindMismatch),
        }
        if self.reply_to_push_id.is_some() && self.kind != PushKind::DirectMessage {
            return Err(PushRecordValidationError::InvalidReplyTarget);
        }

        let is_subscription_activity = self.kind == PushKind::SubscriptionActivity;
        match (
            is_subscription_activity,
            self.body.as_ref(),
            self.activity.as_ref(),
        ) {
            (true, None, Some(activity)) if activity.is_valid() => {
                let PushHeaderFacts::SubscriptionActivity {
                    root_count,
                    held_since,
                    ..
                } = &self.header_facts
                else {
                    return Err(PushRecordValidationError::InvalidActivityFacts);
                };
                if usize::from(*root_count) != activity.ranges.len()
                    || held_since.is_some() != activity.held
                {
                    return Err(PushRecordValidationError::InvalidActivityFacts);
                }
            }
            (false, Some(body), None) => {
                if MessageText::try_from(body.clone()).is_err() {
                    return Err(PushRecordValidationError::InvalidBody);
                }
                if self.kind == PushKind::DirectMessage
                    && body.len() > MAX_DIRECT_MESSAGE_BODY_BYTES
                {
                    return Err(PushRecordValidationError::DirectMessageTooLarge);
                }
            }
            (true, _, _) => return Err(PushRecordValidationError::InvalidActivityContent),
            (false, _, _) => return Err(PushRecordValidationError::InvalidBodyContent),
        }

        Ok(PushRecord {
            push_id: self.push_id,
            kind: self.kind,
            origin: self.origin,
            origin_router_ref: self.origin_router_ref,
            target: self.target,
            reply_to_push_id: self.reply_to_push_id,
            header_facts: self.header_facts,
            body: self.body,
            activity: self.activity,
            delivery_state: PushDeliveryState::Pending,
            last_outcome: None,
            created_at: self.created_at,
            settled_at: None,
            read_at: None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PushDeliveryState {
    Pending,
    Attempted,
    Held,
    Delivered,
    OutcomeUnknown,
    Rejected,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushRecord {
    pub push_id: PushId,
    pub kind: PushKind,
    pub origin: PushOrigin,
    pub origin_router_ref: Option<String>,
    pub target: SessionRef,
    pub reply_to_push_id: Option<PushId>,
    pub header_facts: PushHeaderFacts,
    pub body: Option<String>,
    pub activity: Option<PushActivitySnapshot>,
    pub delivery_state: PushDeliveryState,
    pub last_outcome: Option<DeliveryReceipt>,
    pub created_at: DateTime<Utc>,
    pub settled_at: Option<DateTime<Utc>>,
    pub read_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushRecordShowParams {
    pub caller: SessionRef,
    /// A local push id or a router:// machine/push/id link.
    pub reference: String,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushActivityRangeRead {
    pub range: PushActivityRange,
    pub messages: Vec<message_board::Message>,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushRecordShowResult {
    pub record: PushRecord,
    pub link: String,
    pub activity_ranges: Vec<PushActivityRangeRead>,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushRecordListParams {
    pub caller: SessionRef,
    pub limit: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushRecordHistoryParams {
    pub caller: SessionRef,
    pub with: SessionRef,
    pub limit: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushRecordNotice {
    pub push_id: PushId,
    pub link: String,
    pub line: String,
    pub origin: PushOrigin,
    pub target: SessionRef,
    pub reply_to_push_id: Option<PushId>,
    pub delivery_state: PushDeliveryState,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushRecordListResult {
    pub records: Vec<PushRecordNotice>,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushMessageSendResult {
    pub push_id: PushId,
    pub link: String,
    pub target: SessionRef,
    pub target_identity: String,
    pub receipt: DeliveryReceipt,
}

impl PushRecord {
    pub fn from_storage_parts(
        draft: PushRecordDraft,
        delivery_state: PushDeliveryState,
        last_outcome: Option<DeliveryReceipt>,
        settled_at: Option<DateTime<Utc>>,
        read_at: Option<DateTime<Utc>>,
    ) -> Result<Self, PushRecordValidationError> {
        let mut record = draft.into_pending()?;
        if read_at.is_some() && record.kind != PushKind::DirectMessage {
            return Err(PushRecordValidationError::InvalidReadState);
        }
        let is_settled = matches!(
            delivery_state,
            PushDeliveryState::Delivered
                | PushDeliveryState::OutcomeUnknown
                | PushDeliveryState::Rejected
        );
        if is_settled != (last_outcome.is_some() && settled_at.is_some())
            || (delivery_state == PushDeliveryState::Pending
                && (last_outcome.is_some() || settled_at.is_some()))
            || (delivery_state == PushDeliveryState::Attempted
                && (last_outcome.is_some() || settled_at.is_some()))
            || (delivery_state == PushDeliveryState::Held && settled_at.is_some())
        {
            return Err(PushRecordValidationError::InvalidDeliveryState);
        }
        if let Some(outcome) = last_outcome.as_ref() {
            let outcome_matches_state = match delivery_state {
                PushDeliveryState::Delivered => matches!(
                    outcome.outcome,
                    DeliveryOutcome::Started
                        | DeliveryOutcome::Steered
                        | DeliveryOutcome::StartedOrSteered
                        | DeliveryOutcome::Queued
                        | DeliveryOutcome::PeerMessageWritten
                ),
                PushDeliveryState::OutcomeUnknown => outcome.outcome == DeliveryOutcome::Unknown,
                PushDeliveryState::Held => matches!(
                    outcome.outcome,
                    DeliveryOutcome::NotSubmitted { .. } | DeliveryOutcome::Rejected(_)
                ),
                PushDeliveryState::Rejected => matches!(
                    outcome.outcome,
                    DeliveryOutcome::NotSubmitted { .. } | DeliveryOutcome::Rejected(_)
                ),
                PushDeliveryState::Pending | PushDeliveryState::Attempted => false,
            };
            if !outcome_matches_state {
                return Err(PushRecordValidationError::InvalidDeliveryState);
            }
        }
        record.delivery_state = delivery_state;
        record.last_outcome = last_outcome;
        record.settled_at = settled_at;
        record.read_at = read_at;
        Ok(record)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PushRecordValidationError {
    #[error("push kind and header facts do not agree")]
    HeaderKindMismatch,
    #[error("push kind and origin do not agree")]
    OriginKindMismatch,
    #[error("push origin reference is invalid for this origin")]
    InvalidOriginReference,
    #[error("reply-to is only valid for direct messages")]
    InvalidReplyTarget,
    #[error("direct-message body exceeds 64 KiB")]
    DirectMessageTooLarge,
    #[error("push body is invalid")]
    InvalidBody,
    #[error("push body and activity ranges do not agree with the kind")]
    InvalidBodyContent,
    #[error("subscription-activity requires valid ranges and no body")]
    InvalidActivityContent,
    #[error("subscription activity facts do not match the range snapshot")]
    InvalidActivityFacts,
    #[error("delivery state and outcome do not agree")]
    InvalidDeliveryState,
    #[error("only a direct message may be marked read")]
    InvalidReadState,
}
