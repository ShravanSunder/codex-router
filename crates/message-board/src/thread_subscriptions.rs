//! Durable per-reader Thread and Topic subscription contracts.
mod root_notice;

pub use root_notice::{
    PendingRootNotice, SubscriptionBatch, SubscriptionBatchRootSettlement,
    SubscriptionBatchSettlement,
};

use crate::{Identity, MessageId, TopicId};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use uuid::{Uuid, Variant};

pub const DEFAULT_SUBSCRIPTION_QUIET_SECONDS: u64 = 2 * 60;
pub const DEFAULT_SUBSCRIPTION_CAP_SECONDS: u64 = 10 * 60;
pub const DEFAULT_SUBSCRIPTION_LIFETIME_SECONDS: u64 = 24 * 60 * 60;
pub const MIN_SUBSCRIPTION_QUIET_SECONDS: u64 = 0;
pub const MAX_SUBSCRIPTION_QUIET_SECONDS: u64 = 30 * 60;
pub const MAX_SUBSCRIPTION_CAP_SECONDS: u64 = 60 * 60;
pub const MIN_SUBSCRIPTION_LIFETIME_SECONDS: u64 = 10 * 60;
pub const MAX_SUBSCRIPTION_LIFETIME_SECONDS: u64 = 7 * 24 * 60 * 60;
/// Maximum number of per-root locators selected into one subscription notice.
pub const MAX_SUBSCRIPTION_NOTICE_ROOTS: usize = 20;

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("invalid subscription {field}: {requirement}")]
pub struct InvalidSubscriptionField {
    pub field: &'static str,
    pub requirement: &'static str,
}

fn invalid_subscription_field(
    field: &'static str,
    requirement: &'static str,
) -> InvalidSubscriptionField {
    InvalidSubscriptionField { field, requirement }
}

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SubscriptionScope {
    #[serde(rename_all = "camelCase")]
    Thread { root_message_id: MessageId },
    #[serde(rename_all = "camelCase")]
    Topic { topic_id: TopicId },
}

impl SubscriptionScope {
    #[must_use]
    pub fn thread(root_message_id: MessageId) -> Self {
        Self::Thread { root_message_id }
    }

    #[must_use]
    pub fn topic(topic_id: TopicId) -> Self {
        Self::Topic { topic_id }
    }

    #[must_use]
    pub fn kind_and_id(&self) -> (&'static str, &str) {
        match self {
            Self::Thread { root_message_id } => ("thread", root_message_id.as_str()),
            Self::Topic { topic_id } => ("topic", topic_id.as_str()),
        }
    }

    #[must_use]
    pub fn root_message_id(&self) -> Option<&MessageId> {
        match self {
            Self::Thread { root_message_id } => Some(root_message_id),
            Self::Topic { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SubscriptionMode {
    Deliver,
    Poll,
    Off,
}

impl SubscriptionMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deliver => "deliver",
            Self::Poll => "poll",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum WhenIdle {
    Hold,
    Wake,
    Drop,
}

impl WhenIdle {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Wake => "wake",
            Self::Drop => "drop",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchTiming {
    pub quiet_seconds: u64,
    pub cap_seconds: u64,
}

impl BatchTiming {
    pub fn new(quiet_seconds: u64, cap_seconds: u64) -> Result<Self, InvalidSubscriptionField> {
        if !(MIN_SUBSCRIPTION_QUIET_SECONDS..=MAX_SUBSCRIPTION_QUIET_SECONDS)
            .contains(&quiet_seconds)
        {
            return Err(invalid_subscription_field(
                "quietSeconds",
                "must be between 0 and 1800 seconds",
            ));
        }
        if !(quiet_seconds..=MAX_SUBSCRIPTION_CAP_SECONDS).contains(&cap_seconds) {
            return Err(invalid_subscription_field(
                "capSeconds",
                "must be at least quietSeconds and at most 3600 seconds",
            ));
        }
        Ok(Self {
            quiet_seconds,
            cap_seconds,
        })
    }

    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            quiet_seconds: DEFAULT_SUBSCRIPTION_QUIET_SECONDS,
            cap_seconds: DEFAULT_SUBSCRIPTION_CAP_SECONDS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SubscriptionLifetime(u64);

impl SubscriptionLifetime {
    pub fn new(seconds: u64) -> Result<Self, InvalidSubscriptionField> {
        if !(MIN_SUBSCRIPTION_LIFETIME_SECONDS..=MAX_SUBSCRIPTION_LIFETIME_SECONDS)
            .contains(&seconds)
        {
            return Err(invalid_subscription_field(
                "forSeconds",
                "must be between 600 seconds and 604800 seconds",
            ));
        }
        Ok(Self(seconds))
    }

    #[must_use]
    pub const fn seconds(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn defaults() -> Self {
        Self(DEFAULT_SUBSCRIPTION_LIFETIME_SECONDS)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionPolicy {
    pub mode: SubscriptionMode,
    pub when_idle: WhenIdle,
    pub timing: BatchTiming,
    pub lifetime: SubscriptionLifetime,
}

impl SubscriptionPolicy {
    pub fn new(
        reader: &Identity,
        mode: SubscriptionMode,
        when_idle: WhenIdle,
        timing: BatchTiming,
        lifetime: SubscriptionLifetime,
    ) -> Result<Self, InvalidSubscriptionField> {
        if matches!(reader, Identity::Human { .. }) && mode == SubscriptionMode::Deliver {
            return Err(invalid_subscription_field(
                "mode",
                "deliver requires a session Reader",
            ));
        }
        Ok(Self {
            mode,
            when_idle,
            timing,
            lifetime,
        })
    }

    #[must_use]
    pub fn defaults_for(reader: &Identity) -> Self {
        Self {
            mode: if matches!(reader, Identity::Session { .. }) {
                SubscriptionMode::Deliver
            } else {
                SubscriptionMode::Off
            },
            when_idle: WhenIdle::Hold,
            timing: BatchTiming::defaults(),
            lifetime: SubscriptionLifetime::defaults(),
        }
    }

    pub fn apply_patch(
        &self,
        reader: &Identity,
        patch: &SubscriptionPolicyPatch,
    ) -> Result<Self, InvalidSubscriptionField> {
        let timing = BatchTiming::new(
            patch
                .timing
                .quiet_seconds
                .unwrap_or(self.timing.quiet_seconds),
            patch.timing.cap_seconds.unwrap_or(self.timing.cap_seconds),
        )?;
        let lifetime =
            SubscriptionLifetime::new(patch.lifetime_seconds.unwrap_or(self.lifetime.seconds()))?;
        Self::new(
            reader,
            patch.mode.unwrap_or(self.mode),
            patch.when_idle.unwrap_or(self.when_idle),
            timing,
            lifetime,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionTimingPatch {
    pub quiet_seconds: Option<u64>,
    pub cap_seconds: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionPolicyPatch {
    pub mode: Option<SubscriptionMode>,
    pub when_idle: Option<WhenIdle>,
    #[serde(flatten)]
    pub timing: SubscriptionTimingPatch,
    pub lifetime_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EndReason {
    Left,
    Replaced,
    Resolved,
    Cancelled,
    Expired,
}

impl EndReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Replaced => "replaced",
            Self::Resolved => "resolved",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SubscriptionGeneration(u64);

impl SubscriptionGeneration {
    pub fn new(value: u64) -> Result<Self, InvalidSubscriptionField> {
        if value == 0 {
            return Err(invalid_subscription_field(
                "generation",
                "must be greater than zero",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Result<Self, InvalidSubscriptionField> {
        self.0
            .checked_add(1)
            .ok_or_else(|| invalid_subscription_field("generation", "must not overflow"))
            .and_then(Self::new)
    }
}

macro_rules! uuid_v7_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({
                    "type": "string",
                    "minLength": 36,
                    "maxLength": 36,
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
                })
            }
        }

        impl TryFrom<String> for $name {
            type Error = InvalidSubscriptionField;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                let parsed = Uuid::parse_str(&value).map_err(|_| {
                    invalid_subscription_field(
                        stringify!($name),
                        "must be a canonical lowercase RFC UUIDv7",
                    )
                })?;
                if parsed.get_version_num() != 7
                    || parsed.get_variant() != Variant::RFC4122
                    || parsed.hyphenated().to_string() != value
                {
                    return Err(invalid_subscription_field(
                        stringify!($name),
                        "must be a canonical lowercase RFC UUIDv7",
                    ));
                }
                Ok(Self(value))
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl $name {
            #[must_use]
            pub fn generate() -> Self {
                Self(Uuid::now_v7().hyphenated().to_string())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

uuid_v7_id!(BatchId);
uuid_v7_id!(SubscriptionWindowId);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SubscriptionState {
    Active,
    Draining,
    #[serde(rename_all = "camelCase")]
    Ended {
        reason: EndReason,
    },
}

impl SubscriptionState {
    #[must_use]
    pub const fn end_reason(self) -> Option<EndReason> {
        match self {
            Self::Ended { reason } => Some(reason),
            Self::Active | Self::Draining => None,
        }
    }

    #[must_use]
    pub const fn is_delivery_eligible(self) -> bool {
        matches!(self, Self::Active | Self::Draining)
    }
}

/// Durable delivery result evidence retained on a subscription record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SubscriptionDeliveryOutcome {
    Accepted,
    Unknown { evidence: serde_json::Value },
    Rejected { evidence: serde_json::Value },
    NotSubmitted { reason: String, retryable: bool },
    Dropped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionRootRecord {
    pub root_message_id: MessageId,
    pub opened_at: DateTime<Utc>,
    pub last_arrival_at: DateTime<Utc>,
    pub pending_count: u64,
    pub held_since: Option<DateTime<Utc>>,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub retry_attempts: u32,
}

impl SubscriptionRootRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root_message_id: MessageId,
        opened_at: DateTime<Utc>,
        last_arrival_at: DateTime<Utc>,
        pending_count: u64,
        held_since: Option<DateTime<Utc>>,
        next_retry_at: Option<DateTime<Utc>>,
        retry_attempts: u32,
    ) -> Result<Self, InvalidSubscriptionField> {
        if last_arrival_at < opened_at {
            return Err(invalid_subscription_field(
                "lastArrivalAt",
                "must not be earlier than openedAt",
            ));
        }
        Ok(Self {
            root_message_id,
            opened_at,
            last_arrival_at,
            pending_count,
            held_since,
            next_retry_at,
            retry_attempts,
        })
    }
}

/// Stored subscription facts plus the windows currently covered by its scope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSubscriptionRecord {
    pub reader: Identity,
    pub scope: SubscriptionScope,
    pub policy: SubscriptionPolicy,
    pub state: SubscriptionState,
    pub renewed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub generation: SubscriptionGeneration,
    pub last_outcome: Option<SubscriptionDeliveryOutcome>,
    pub roots: Vec<SubscriptionRootRecord>,
}

impl ThreadSubscriptionRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reader: Identity,
        scope: SubscriptionScope,
        policy: SubscriptionPolicy,
        state: SubscriptionState,
        renewed_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
        ended_at: Option<DateTime<Utc>>,
        generation: SubscriptionGeneration,
        last_outcome: Option<SubscriptionDeliveryOutcome>,
        roots: Vec<SubscriptionRootRecord>,
    ) -> Result<Self, InvalidSubscriptionField> {
        if expires_at <= renewed_at {
            return Err(invalid_subscription_field(
                "expiresAt",
                "must be later than renewedAt",
            ));
        }
        if state.end_reason().is_some() != ended_at.is_some() {
            return Err(invalid_subscription_field(
                "endedAt",
                "must be set exactly when state is ended",
            ));
        }
        if matches!(scope, SubscriptionScope::Thread { .. }) && roots.len() > 1 {
            return Err(invalid_subscription_field(
                "roots",
                "a Thread subscription can contain at most one root window",
            ));
        }
        Ok(Self {
            reader,
            scope,
            policy,
            state,
            renewed_at,
            expires_at,
            ended_at,
            generation,
            last_outcome,
            roots,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSubscriptionSubscribeRequest {
    pub reader: Identity,
    pub scope: SubscriptionScope,
    pub policy: SubscriptionPolicyPatch,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSubscriptionUnsubscribeRequest {
    pub reader: Identity,
    pub scope: SubscriptionScope,
}
