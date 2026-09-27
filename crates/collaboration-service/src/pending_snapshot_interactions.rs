//! Identifies interaction requests that remain unresolved in one attached hub snapshot.
use crate::HubEvent;
use session_event_model::SessionEvent;
use std::collections::HashSet;

pub(crate) fn pending_snapshot_requests(snapshot: &[HubEvent]) -> HashSet<String> {
    let mut pending = HashSet::new();
    for event in snapshot {
        match &event.event {
            SessionEvent::InteractionRequested { interaction } => {
                pending.insert(interaction.request_id().to_owned());
            }
            SessionEvent::InteractionResolved { request_id } => {
                pending.remove(request_id);
            }
            _ => {}
        }
    }
    pending
}
