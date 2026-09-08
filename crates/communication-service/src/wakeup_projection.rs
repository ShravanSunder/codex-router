//! Domain timestamps and records become the language-independent public wake snapshot here.
use agent_automation::WakeRecord;
use communication_protocol::{
    FireKind, FireReceipt, ObservationTimestamp, SavedMessage, UuidIdentity, WakeDefinition,
    WakeSnapshot, WakeState,
};

pub(crate) fn snapshot(
    record: WakeRecord<SavedMessage>,
    service_id: &UuidIdentity,
    observed_at_ms: i64,
) -> Result<WakeSnapshot, ()> {
    let definition = record.definition;
    let first_fire = record
        .first_fire
        .map(|fire| {
            Ok(FireReceipt {
                kind: FireKind::WakeFired,
                wakeup_id: fire.wakeup_id,
                occurrence_id: fire.occurrence_id,
                due_at: timestamp(fire.due_at_ms)?,
                fired_at: timestamp(fire.fired_at_ms)?,
            })
        })
        .transpose()?;
    let state = match record.state {
        agent_automation::WakeState::Active => WakeState::Active,
        agent_automation::WakeState::Paused => WakeState::Paused,
        agent_automation::WakeState::Cancelled => WakeState::Cancelled,
        agent_automation::WakeState::Expired => WakeState::Expired,
        agent_automation::WakeState::Finished => WakeState::Finished,
    };
    // Cursor contents are opaque to SDK callers and bound to the selected service.
    let cursor = serde_json::to_string(&(
        1_u8,
        service_id,
        "automation-events",
        record.latest_event_sequence,
        observed_at_ms,
    ))
    .map_err(|_| ())?;
    Ok(WakeSnapshot {
        definition: WakeDefinition {
            wakeup_id: definition.wakeup_id,
            change_id: definition.change_id,
            message: definition.message,
            timing: serde_json::from_value(
                serde_json::to_value(definition.timing).map_err(|_| ())?,
            )
            .map_err(|_| ())?,
            anchor_at: timestamp(definition.anchor_at_ms)?,
            expires_at: definition.expires_at_ms.map(timestamp).transpose()?,
            created_at: timestamp(definition.created_at_ms)?,
        },
        state,
        next_due_at: record.next_due_at_ms.map(timestamp).transpose()?,
        first_fire,
        pending_delivery_id: record.pending_delivery_id,
        latest_event_cursor: cursor,
    })
}
pub(crate) fn timestamp(value: i64) -> Result<ObservationTimestamp, ()> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value)
        .ok_or(())?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        .try_into()
        .map_err(|_| ())
}
