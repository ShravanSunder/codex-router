//! Public schedule snapshots project separate configuration/timing and Run-owned occupancy.
use automation_storage::ScheduleInspection;
use communication_protocol::{EndpointRef, ScheduleSnapshot, SessionRef};
pub(crate) fn snapshot(
    inspection: ScheduleInspection<SessionRef, EndpointRef>,
) -> Result<ScheduleSnapshot, ()> {
    let record = inspection.record;
    Ok(ScheduleSnapshot {
        schedule_id: record.schedule_id,
        change_id: record.change_id,
        definition: serde_json::from_value(
            serde_json::to_value(record.definition).map_err(|_| ())?,
        )
        .map_err(|_| ())?,
        imported_continuity: serde_json::from_value(
            serde_json::to_value(record.imported_continuity).map_err(|_| ())?,
        )
        .map_err(|_| ())?,
        anchor_at: crate::wakeup_projection::timestamp(record.anchor_at_ms)?,
        next_due_at: record
            .next_due_at_ms
            .map(crate::wakeup_projection::timestamp)
            .transpose()?,
        active_run_id: inspection.active_run_id,
        waiting_run_id: inspection.waiting_run_id,
        created_at: crate::wakeup_projection::timestamp(record.created_at_ms)?,
        updated_at: crate::wakeup_projection::timestamp(record.updated_at_ms)?,
    })
}
