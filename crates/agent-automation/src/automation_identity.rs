//! Stable automation identifiers are separate from native endpoint/thread identities.
use serde::{Deserialize, Serialize};
use uuid::{Uuid, Variant};

#[derive(Debug, thiserror::Error)]
#[error(
    "automation identity parameter {parameter} must be a canonical lowercase RFC UUIDv7, for example 019f0000-0000-7000-8000-000000000001"
)]
pub struct AutomationIdentityError {
    parameter: &'static str,
}

macro_rules! automation_identity {
    ($($name:ident => $parameter:literal),+ $(,)?) => {$ (
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
                let parsed = Uuid::parse_str(&value).map_err(|_| AutomationIdentityError {
                    parameter: $parameter,
                })?;
                if parsed.get_version_num() != 7
                    || parsed.get_variant() != Variant::RFC4122
                    || parsed.hyphenated().to_string() != value
                {
                    return Err(AutomationIdentityError {
                        parameter: $parameter,
                    });
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
    ScheduleId => "scheduleId",
    InstructionId => "instructionId",
    RunId => "runId",
    WakeupId => "wakeupId",
    DeliveryId => "deliveryId",
    ThreadBindingId => "threadBindingId",
    OperationId => "operationId",
    RevisionId => "revisionId",
    OccurrenceId => "occurrenceId",
    AttemptId => "attemptId",
    ChangeId => "changeId",
    EventId => "eventId",
    SubscriptionId => "subscriptionId"
);

#[cfg(test)]
mod tests {
    use super::{AutomationIdentityError, WakeupId};

    #[test]
    fn invalid_automation_identity_names_the_parameter_and_uuid_v7_example() {
        let error = WakeupId::try_from("not-an-identity".to_owned())
            .expect_err("invalid identity is rejected");

        assert_eq!(
            error.to_string(),
            "automation identity parameter wakeupId must be a canonical lowercase RFC UUIDv7, for example 019f0000-0000-7000-8000-000000000001"
        );
        let _error_type_is_public = std::any::type_name::<AutomationIdentityError>();
    }
}
