//! Nonblocking publication boundary for Router-owned Session events.

use session_event_model::SessionEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventSinkOverflow;

pub trait SessionEventSink: Send + Sync + 'static {
    /// Return promptly; a full downstream channel is reported, never waited on.
    fn publish(&self, session_id: &str, event: SessionEvent) -> Result<(), EventSinkOverflow>;
}
