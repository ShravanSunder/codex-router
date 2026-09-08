//! Stable automation identifiers are separate from native endpoint/thread identities.
use serde::{Deserialize, Serialize};
use uuid::{Uuid, Variant};

#[derive(Debug, thiserror::Error)]
#[error("automation identity must be a canonical lowercase RFC UUIDv7")]
pub struct AutomationIdentityError;

macro_rules! automation_identity {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static,str> { stringify!($name).into() }
            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({"type":"string","minLength":36,"maxLength":36,
                    "pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"})
            }
        }
        impl TryFrom<String> for $name {
            type Error = AutomationIdentityError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                let parsed = Uuid::parse_str(&value).map_err(|_| AutomationIdentityError)?;
                if parsed.get_version_num() != 7
                    || parsed.get_variant() != Variant::RFC4122
                    || parsed.hyphenated().to_string() != value
                {
                    return Err(AutomationIdentityError);
                }
                Ok(Self(value))
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self { value.0 }
        }
        impl $name {
            #[must_use]
            pub fn generate() -> Self { Self(Uuid::now_v7().hyphenated().to_string()) }
            #[must_use]
            pub fn as_str(&self) -> &str { &self.0 }
        }
    )+};
}
automation_identity!(
    ScheduleId,
    InstructionId,
    RunId,
    WakeupId,
    DeliveryId,
    ThreadBindingId,
    OperationId,
    RevisionId,
    OccurrenceId,
    AttemptId,
    ChangeId,
    EventId
);
