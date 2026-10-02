//! Canonical typed identities for Router-authored push events.

use crate::{NonEmptyText, PushKind, SessionRef};
use agent_automation::{OccurrenceId, RunId, ScheduleId, WakeupId};
use message_board::{BatchId, SubscriptionGeneration, SubscriptionScope};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::borrow::Cow;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouterOriginRef {
    Wake {
        wakeup_id: WakeupId,
        occurrence_id: OccurrenceId,
    },
    ScheduleRun {
        schedule_id: ScheduleId,
        run_id: RunId,
    },
    Interaction {
        interaction_id: InteractionId,
        presentation_id: InteractionPresentationId,
    },
    SubscriptionActivity {
        target: SessionRef,
        batch_id: BatchId,
    },
    SubscriptionExpiry {
        target: SessionRef,
        scope: SubscriptionScope,
        subscription_generation: SubscriptionGeneration,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InteractionId(String);

impl TryFrom<String> for InteractionId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        NonEmptyText::try_from(value.clone())?;
        Ok(Self(value))
    }
}

impl From<InteractionId> for String {
    fn from(value: InteractionId) -> Self {
        value.0
    }
}

impl InteractionId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl JsonSchema for InteractionId {
    fn schema_name() -> Cow<'static, str> {
        "InteractionId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string","minLength":1,"maxLength":4096})
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InteractionPresentationId(String);

impl InteractionPresentationId {
    #[must_use]
    pub fn generate() -> Self {
        Self(uuid::Uuid::now_v7().hyphenated().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for InteractionPresentationId {
    type Error = crate::PushIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let uuid = uuid::Uuid::parse_str(&value).map_err(|_| crate::PushIdError::PushId)?;
        if uuid.get_version_num() != 7 || value != uuid.hyphenated().to_string() {
            return Err(crate::PushIdError::PushId);
        }
        Ok(Self(value))
    }
}

impl From<InteractionPresentationId> for String {
    fn from(value: InteractionPresentationId) -> Self {
        value.0
    }
}

impl JsonSchema for InteractionPresentationId {
    fn schema_name() -> Cow<'static, str> {
        "InteractionPresentationId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "minLength": 36,
            "maxLength": 36,
            "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum RouterOriginRefWire {
    Wake {
        wakeup_id: WakeupId,
        occurrence_id: OccurrenceId,
    },
    ScheduleRun {
        schedule_id: ScheduleId,
        run_id: RunId,
    },
    Interaction {
        interaction_id: InteractionId,
        presentation_id: InteractionPresentationId,
    },
    SubscriptionActivity {
        target: SessionRef,
        batch_id: BatchId,
    },
    SubscriptionExpiry {
        target: SessionRef,
        scope: SubscriptionScope,
        subscription_generation: SubscriptionGeneration,
    },
}

impl RouterOriginRef {
    #[must_use]
    pub const fn supports_kind(&self, kind: PushKind) -> bool {
        match self {
            Self::Wake { .. } => matches!(kind, PushKind::Wake),
            Self::ScheduleRun { .. } => matches!(kind, PushKind::ScheduleRun),
            Self::Interaction { .. } => {
                matches!(kind, PushKind::Approval | PushKind::Question)
            }
            Self::SubscriptionActivity { .. } => matches!(kind, PushKind::SubscriptionActivity),
            Self::SubscriptionExpiry { .. } => matches!(kind, PushKind::SubscriptionExpiry),
        }
    }

    pub fn canonical_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.to_wire())
    }

    pub fn parse_canonical(encoded: &str) -> Result<Self, serde_json::Error> {
        let wire: RouterOriginRefWire = serde_json::from_str(encoded)?;
        let canonical = serde_json::to_string(&wire)?;
        if encoded != canonical {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "Router origin reference is not canonical",
            ));
        }
        Ok(Self::from_wire(wire))
    }

    fn to_wire(&self) -> RouterOriginRefWire {
        match self {
            Self::Wake {
                wakeup_id,
                occurrence_id,
            } => RouterOriginRefWire::Wake {
                wakeup_id: wakeup_id.clone(),
                occurrence_id: occurrence_id.clone(),
            },
            Self::ScheduleRun {
                schedule_id,
                run_id,
            } => RouterOriginRefWire::ScheduleRun {
                schedule_id: schedule_id.clone(),
                run_id: run_id.clone(),
            },
            Self::Interaction {
                interaction_id,
                presentation_id,
            } => RouterOriginRefWire::Interaction {
                interaction_id: interaction_id.clone(),
                presentation_id: presentation_id.clone(),
            },
            Self::SubscriptionActivity { target, batch_id } => {
                RouterOriginRefWire::SubscriptionActivity {
                    target: target.clone(),
                    batch_id: batch_id.clone(),
                }
            }
            Self::SubscriptionExpiry {
                target,
                scope,
                subscription_generation,
            } => RouterOriginRefWire::SubscriptionExpiry {
                target: target.clone(),
                scope: scope.clone(),
                subscription_generation: *subscription_generation,
            },
        }
    }

    fn from_wire(wire: RouterOriginRefWire) -> Self {
        match wire {
            RouterOriginRefWire::Wake {
                wakeup_id,
                occurrence_id,
            } => Self::Wake {
                wakeup_id,
                occurrence_id,
            },
            RouterOriginRefWire::ScheduleRun {
                schedule_id,
                run_id,
            } => Self::ScheduleRun {
                schedule_id,
                run_id,
            },
            RouterOriginRefWire::Interaction {
                interaction_id,
                presentation_id,
            } => Self::Interaction {
                interaction_id,
                presentation_id,
            },
            RouterOriginRefWire::SubscriptionActivity { target, batch_id } => {
                Self::SubscriptionActivity { target, batch_id }
            }
            RouterOriginRefWire::SubscriptionExpiry {
                target,
                scope,
                subscription_generation,
            } => Self::SubscriptionExpiry {
                target,
                scope,
                subscription_generation,
            },
        }
    }
}

impl Serialize for RouterOriginRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded = self.canonical_string().map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&encoded)
    }
}

impl<'de> Deserialize<'de> for RouterOriginRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        Self::parse_canonical(&encoded).map_err(D::Error::custom)
    }
}

impl JsonSchema for RouterOriginRef {
    fn schema_name() -> Cow<'static, str> {
        "RouterOriginRef".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string","minLength":2,"maxLength":1024})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_origin_variant_round_trips_through_its_canonical_string() {
        let target: SessionRef = serde_json::from_value(json!({
            "endpoint": {
                "serviceId": "018f47d2-24d5-7a68-b9ec-6f759c39458f",
                "endpointId": "codex-local"
            },
            "sessionId": "018f47d2-24d5-7a68-b9ec-6f759c394590"
        }))
        .expect("target session");
        let scope: SubscriptionScope = serde_json::from_value(json!({
            "kind": "thread",
            "rootMessageId": "018f47d2-24d5-7a68-b9ec-6f759c394591"
        }))
        .expect("subscription scope");
        let references = [
            RouterOriginRef::Wake {
                wakeup_id: WakeupId::generate(),
                occurrence_id: OccurrenceId::generate(),
            },
            RouterOriginRef::ScheduleRun {
                schedule_id: ScheduleId::generate(),
                run_id: RunId::generate(),
            },
            RouterOriginRef::Interaction {
                interaction_id: InteractionId::try_from("permission-request-7".to_owned())
                    .expect("interaction id"),
                presentation_id: InteractionPresentationId::generate(),
            },
            RouterOriginRef::SubscriptionActivity {
                target: target.clone(),
                batch_id: BatchId::generate(),
            },
            RouterOriginRef::SubscriptionExpiry {
                target,
                scope,
                subscription_generation: SubscriptionGeneration::new(3)
                    .expect("subscription generation"),
            },
        ];

        for reference in references {
            let canonical = reference.canonical_string().expect("canonical origin");
            assert_eq!(
                RouterOriginRef::parse_canonical(&canonical).expect("parsed origin"),
                reference
            );
            let encoded = serde_json::to_string(&reference).expect("serialized origin");
            assert_eq!(
                serde_json::from_str::<RouterOriginRef>(&encoded).expect("decoded origin"),
                reference
            );
        }
    }

    #[test]
    fn origin_parser_rejects_noncanonical_unknown_and_wrong_identity_values() {
        let schedule_run = RouterOriginRef::ScheduleRun {
            schedule_id: ScheduleId::generate(),
            run_id: RunId::generate(),
        };
        let canonical = schedule_run.canonical_string().expect("canonical origin");
        assert!(RouterOriginRef::parse_canonical(&canonical.replace(',', ", ")).is_err());
        assert!(RouterOriginRef::parse_canonical(r#"{"kind":"future"}"#).is_err());
        assert!(InteractionId::try_from(String::new()).is_err());
        assert!(InteractionPresentationId::try_from("not-a-uuid".to_owned()).is_err());
    }
}
