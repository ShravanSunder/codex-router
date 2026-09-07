//! Per-observer read/event ordering; late completions cannot revive retired coverage.
use crate::JournalError;
use communication_protocol::{ObservationOrdering, ObservationScope, SessionId};
use std::collections::BTreeMap;

pub struct InventoryReconciliation {
    scope: ObservationScope,
    connected: bool,
    threads: BTreeMap<SessionId, ThreadReadState>,
    next_read: u64,
}
#[derive(Default)]
struct ThreadReadState {
    revision: u64,
    pending: Option<u64>,
    has_read: bool,
    conflicts: u8,
}
pub struct ReadTicket {
    scope: ObservationScope,
    thread: SessionId,
    read: u64,
    revision: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ReadDisposition {
    Established,
    Ambiguous { retry: bool },
    Discard,
}
impl InventoryReconciliation {
    pub fn new(scope: ObservationScope) -> Result<Self, JournalError> {
        if scope.generation.is_none() {
            return Err(JournalError::InvalidRecord);
        }
        Ok(Self {
            scope,
            connected: true,
            threads: BTreeMap::new(),
            next_read: 0,
        })
    }
    pub fn begin_read(&mut self, thread: SessionId) -> Result<ReadTicket, JournalError> {
        if !self.connected {
            return Err(JournalError::InvalidRecord);
        }
        if !self.threads.contains_key(&thread) && self.threads.len() >= 16384 {
            return Err(JournalError::Capacity);
        }
        let state = self.threads.entry(thread.clone()).or_default();
        if state.pending.is_some() {
            return Err(JournalError::InvalidRecord);
        }
        let read = self.next_read;
        self.next_read = read.checked_add(1).ok_or(JournalError::Capacity)?;
        state.pending = Some(read);
        state.has_read = true;
        Ok(ReadTicket {
            scope: self.scope.clone(),
            thread,
            read,
            revision: state.revision,
        })
    }
    pub fn notification(
        &mut self,
        thread: &SessionId,
    ) -> Result<ObservationOrdering, JournalError> {
        if !self.connected {
            return Err(JournalError::InvalidRecord);
        }
        if !self.threads.contains_key(thread) && self.threads.len() >= 16384 {
            return Err(JournalError::Capacity);
        }
        let state = self.threads.entry(thread.clone()).or_default();
        state.revision = state
            .revision
            .checked_add(1)
            .ok_or(JournalError::Capacity)?;
        // Without a native ordering token, an event after an interleaved read may be delayed.
        Ok(if state.pending.is_some() || state.has_read {
            ObservationOrdering::Ambiguous
        } else {
            ObservationOrdering::Established
        })
    }
    pub fn finish_read(&mut self, ticket: ReadTicket) -> ReadDisposition {
        if !self.connected || ticket.scope != self.scope {
            return ReadDisposition::Discard;
        }
        let Some(state) = self.threads.get_mut(&ticket.thread) else {
            return ReadDisposition::Discard;
        };
        if state.pending != Some(ticket.read) {
            return ReadDisposition::Discard;
        }
        state.pending = None;
        if state.revision != ticket.revision {
            state.conflicts = state.conflicts.saturating_add(1);
            ReadDisposition::Ambiguous {
                retry: state.conflicts < 3,
            }
        } else {
            state.conflicts = 0;
            ReadDisposition::Established
        }
    }
    pub fn disconnect(&mut self) {
        self.connected = false;
        self.threads.clear();
    }
}
