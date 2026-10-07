use super::{InteractionHistoryError, InteractionHistoryRecord};
use chrono::{DateTime, Utc};
use std::{collections::BTreeMap, path::Path};

#[derive(Default)]
pub(in crate::interaction_broker) struct ParsedInteractionHistory {
    pub(super) records: BTreeMap<String, ParsedInteractionHistoryRow>,
}

pub(super) struct ParsedInteractionHistoryRow {
    pub(super) record: InteractionHistoryRecord,
    pub(super) created_at: Option<DateTime<Utc>>,
}

pub(in crate::interaction_broker) async fn parse_interaction_history_file(
    path: &Path,
) -> Result<ParsedInteractionHistory, InteractionHistoryError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ParsedInteractionHistory::default());
        }
        Err(_) => return Err(InteractionHistoryError::Unavailable),
    };
    parse_interaction_history_bytes(&bytes)
}

fn parse_interaction_history_bytes(
    bytes: &[u8],
) -> Result<ParsedInteractionHistory, InteractionHistoryError> {
    let stored_values: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(bytes).map_err(|_| InteractionHistoryError::Unavailable)?;
    let mut records = BTreeMap::new();
    for (request_id, mut value) in stored_values {
        let object = value
            .as_object_mut()
            .ok_or(InteractionHistoryError::Unavailable)?;
        let created_at = match object.remove("createdAt") {
            Some(serde_json::Value::String(value)) => Some(parse_history_timestamp(&value)?),
            None => None,
            Some(_) => return Err(InteractionHistoryError::Unavailable),
        };
        let record: InteractionHistoryRecord =
            serde_json::from_value(value).map_err(|_| InteractionHistoryError::Unavailable)?;
        if request_id != record.request_id() || !record.is_valid_stored_value() {
            return Err(InteractionHistoryError::Unavailable);
        }
        records.insert(
            request_id,
            ParsedInteractionHistoryRow { record, created_at },
        );
    }
    Ok(ParsedInteractionHistory { records })
}

fn parse_history_timestamp(value: &str) -> Result<DateTime<Utc>, InteractionHistoryError> {
    if !value.ends_with('Z') {
        return Err(InteractionHistoryError::Unavailable);
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| InteractionHistoryError::Unavailable)
}
