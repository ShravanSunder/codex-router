//! Exact worker output source retained across summary attempts and later continuity.
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SummarySourceReference {
    NativeTurn { turn_id: String },
    ProviderOperation { attempt_id: crate::AttemptId },
}

impl SummarySourceReference {
    #[must_use]
    pub fn native_turn_id(&self) -> Option<&str> {
        match self {
            Self::NativeTurn { turn_id } => Some(turn_id),
            Self::ProviderOperation { .. } => None,
        }
    }
}

pub(crate) fn deserialize_stored_source_reference<'de, TDeserializer>(
    deserializer: TDeserializer,
) -> Result<SummarySourceReference, TDeserializer::Error>
where
    TDeserializer: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StoredReference {
        Current(SummarySourceReference),
        LegacyNative(String),
    }

    match StoredReference::deserialize(deserializer)? {
        StoredReference::Current(reference) => Ok(reference),
        StoredReference::LegacyNative(turn_id) if !turn_id.is_empty() => {
            Ok(SummarySourceReference::NativeTurn { turn_id })
        }
        StoredReference::LegacyNative(_) => Err(serde::de::Error::custom("empty native turn ID")),
    }
}
