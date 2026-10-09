//! Instruction documents project to their public snapshot with RFC 3339 millisecond times.
use agent_automation::InstructionDocument;
use collaboration_protocol::{InstructionSnapshot, ObservationTimestamp};

pub(crate) fn snapshot(document: InstructionDocument) -> Result<InstructionSnapshot, ()> {
    fn time(value: i64) -> Result<ObservationTimestamp, ()> {
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value)
            .ok_or(())?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            .try_into()
            .map_err(|_| ())
    }
    Ok(InstructionSnapshot {
        instruction_id: document.instruction_id,
        revision_id: document.revision_id,
        text: document.text,
        created_at: time(document.created_at_ms)?,
        updated_at: time(document.updated_at_ms)?,
    })
}
