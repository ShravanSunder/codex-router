//! Public metadata projection for stored conversation operations.
use crate::ProviderOperationRecord;
use collaboration_protocol::{ConversationOperationSnapshot, ObservationTimestamp};

pub fn conversation_operation_snapshot(
    record: ProviderOperationRecord,
) -> Result<ConversationOperationSnapshot, &'static str> {
    Ok(ConversationOperationSnapshot {
        operation_id: record.operation_id,
        operation: record.operation_kind,
        binding: record.binding,
        target: record.target,
        stage: record.stage,
        effect: record.effect,
        reconciliation: record.reconciliation_state,
        queue_state: None,
        admitted_at: timestamp_from_millis(record.admitted_at_ms)?,
        terminal_at: record
            .terminal_at_ms
            .map(timestamp_from_millis)
            .transpose()?,
    })
}

fn timestamp_from_millis(milliseconds: i64) -> Result<ObservationTimestamp, &'static str> {
    let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(milliseconds)
        .ok_or("conversation operation timestamp is outside the supported range")?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    ObservationTimestamp::try_from(timestamp)
}
