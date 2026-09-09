//! Event cursors retain the existing sequence/time encoding and are scoped to one service.
use automation_storage::EventPosition;
use communication_protocol::UuidIdentity;
pub(crate) fn encode(service: &UuidIdentity, position: &EventPosition) -> Result<String, ()> {
    serde_json::to_string(&(
        1_u8,
        service,
        "automation-events",
        position.sequence,
        position.observed_at_ms,
    ))
    .map_err(|_| ())
}
pub(crate) fn decode(service: &UuidIdentity, value: &str) -> Result<EventPosition, ()> {
    if value.len() > 4096 {
        return Err(());
    }
    let (version, selected, collection, sequence, observed): (u8, UuidIdentity, String, i64, i64) =
        serde_json::from_str(value).map_err(|_| ())?;
    if version != 1
        || selected != *service
        || collection != "automation-events"
        || sequence < 0
        || observed < 0
    {
        return Err(());
    }
    Ok(EventPosition {
        sequence,
        observed_at_ms: observed,
    })
}
