//! Duplicate-rejecting, validated durable record decoding.
use super::{InteractionHistoryRecord, parse_history_timestamp};
use chrono::{DateTime, Utc};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use std::fmt;

use super::interaction_history_database::HistoryStorageFailure;

struct UniqueJsonValue(Value);

impl<'de> Deserialize<'de> for UniqueJsonValue {
    fn deserialize<DeserializerType: Deserializer<'de>>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error> {
        struct JsonValueVisitor;
        impl<'de> Visitor<'de> for JsonValueVisitor {
            type Value = UniqueJsonValue;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON with unique object keys")
            }
            fn visit_map<MapType: MapAccess<'de>>(
                self,
                mut map: MapType,
            ) -> Result<Self::Value, MapType::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, UniqueJsonValue>()? {
                    if values.insert(key, value.0).is_some() {
                        return Err(de::Error::custom("duplicate object key"));
                    }
                }
                Ok(UniqueJsonValue(Value::Object(values)))
            }
            fn visit_seq<Sequence: SeqAccess<'de>>(
                self,
                mut sequence: Sequence,
            ) -> Result<Self::Value, Sequence::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueJsonValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueJsonValue(Value::Array(values)))
            }
            fn visit_bool<ErrorType: de::Error>(
                self,
                value: bool,
            ) -> Result<Self::Value, ErrorType> {
                Ok(UniqueJsonValue(Value::Bool(value)))
            }
            fn visit_i64<ErrorType: de::Error>(self, value: i64) -> Result<Self::Value, ErrorType> {
                Ok(UniqueJsonValue(value.into()))
            }
            fn visit_u64<ErrorType: de::Error>(self, value: u64) -> Result<Self::Value, ErrorType> {
                Ok(UniqueJsonValue(value.into()))
            }
            fn visit_f64<ErrorType: de::Error>(self, value: f64) -> Result<Self::Value, ErrorType> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueJsonValue(number.into()))
                    .ok_or_else(|| de::Error::custom("nonfinite number"))
            }
            fn visit_str<ErrorType: de::Error>(
                self,
                value: &str,
            ) -> Result<Self::Value, ErrorType> {
                Ok(UniqueJsonValue(Value::String(value.into())))
            }
            fn visit_string<ErrorType: de::Error>(
                self,
                value: String,
            ) -> Result<Self::Value, ErrorType> {
                Ok(UniqueJsonValue(Value::String(value)))
            }
            fn visit_unit<ErrorType: de::Error>(self) -> Result<Self::Value, ErrorType> {
                Ok(UniqueJsonValue(Value::Null))
            }
        }
        deserializer.deserialize_any(JsonValueVisitor)
    }
}

pub(super) fn decode_stored_record(
    request_id: &str,
    record_json: &str,
    created_at: &str,
) -> Result<(InteractionHistoryRecord, DateTime<Utc>), HistoryStorageFailure> {
    let UniqueJsonValue(value) = serde_json::from_str(record_json)
        .map_err(|_| HistoryStorageFailure::InvalidStoredRecord)?;
    let record: InteractionHistoryRecord =
        serde_json::from_value(value).map_err(|_| HistoryStorageFailure::InvalidStoredRecord)?;
    let timestamp = parse_history_timestamp(created_at)
        .map_err(|_| HistoryStorageFailure::InvalidStoredRecord)?;
    if record.request_id() != request_id || !record.is_valid_stored_value() {
        return Err(HistoryStorageFailure::InvalidStoredRecord);
    }
    Ok((record, timestamp))
}
