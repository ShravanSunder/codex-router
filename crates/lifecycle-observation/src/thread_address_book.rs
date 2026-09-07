//! Historical address reducer; public records belong to the protocol package.
use crate::JournalError;
use communication_protocol::{
    AddressEntry, ArchiveState, Existence, JournalPosition, LifecycleChange, LifecycleDisposition,
    LifecycleObservation, LifecycleSubject, ObservationOrdering, StatusOrdering,
};
pub(crate) fn reduce_address_observation(
    previous: Option<AddressEntry>,
    observation: &LifecycleObservation,
    position: JournalPosition,
) -> Result<AddressEntry, JournalError> {
    let LifecycleSubject::Thread { address } = &observation.subject else {
        return Err(JournalError::InvalidRecord);
    };
    let mut entry = previous.unwrap_or_else(|| AddressEntry {
        address: address.clone(),
        first_observed_at: observation.observed_at.clone(),
        last_observed_at: observation.observed_at.clone(),
        last_observation: position.clone(),
        last_status: None,
        status_scope: None,
        status_ordering: StatusOrdering::Unknown,
        disposition: LifecycleDisposition {
            existence: Existence::Unknown,
            archive: ArchiveState::Unknown,
            last_close: None,
            last_change: None,
        },
    });
    if entry.address != *address || entry.last_observation.journal_id != position.journal_id {
        return Err(JournalError::InvalidStorage);
    }
    entry.last_observed_at = observation.observed_at.clone();
    entry.last_observation = position.clone();
    match &observation.change {
        LifecycleChange::ThreadDiscovered => {
            if entry.disposition.existence == Existence::Unknown {
                entry.disposition.existence = Existence::Observed;
            }
        }
        LifecycleChange::ThreadStatus { status, ordering } => {
            entry.last_status = Some(status.clone());
            if *ordering == ObservationOrdering::Established
                && entry.disposition.existence != Existence::Deleted
            {
                entry.status_scope = Some(observation.scope.clone());
                entry.status_ordering = StatusOrdering::Established;
            } else {
                entry.status_scope = None;
                entry.status_ordering = StatusOrdering::Ambiguous;
            }
        }
        LifecycleChange::ThreadArchived => {
            entry.disposition.archive = ArchiveState::Archived;
            entry.disposition.last_change = Some(position);
        }
        LifecycleChange::ThreadUnarchived => {
            entry.disposition.archive = ArchiveState::Unarchived;
            entry.disposition.last_change = Some(position);
        }
        LifecycleChange::ThreadClosed => {
            entry.disposition.last_close = Some(position);
            entry.status_scope = None;
        }
        LifecycleChange::ThreadDeleted => {
            entry.disposition.existence = Existence::Deleted;
            entry.disposition.last_change = Some(position);
            entry.status_scope = None;
        }
        LifecycleChange::TurnTerminal { .. } => {}
        _ => return Err(JournalError::InvalidRecord),
    }
    Ok(entry)
}
