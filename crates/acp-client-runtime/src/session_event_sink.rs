//! Nonblocking publication boundary for Router-owned Session events.

use std::{future::Future, pin::Pin};

use session_event_model::SessionEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session event consumer closed")]
pub struct EventSinkClosed;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session history replay could not begin")]
pub struct HistoryReplayUnavailable;

pub type HistoryReplayFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), HistoryReplayUnavailable>> + Send + 'a>>;

pub trait SessionEventSink: Send + Sync + 'static {
    /// Reset hub history before the agent can publish replayed updates.
    fn begin_history_replay(&self, session_id: &str) -> HistoryReplayFuture<'_>;

    /// Return promptly; failure means the event consumer has closed.
    fn publish(&self, session_id: &str, event: SessionEvent) -> Result<(), EventSinkClosed>;
}

#[cfg(test)]
mod tests {
    use super::{EventSinkClosed, SessionEventSink};
    use session_event_model::SessionEvent;

    struct ReplaySink;

    impl SessionEventSink for ReplaySink {
        fn begin_history_replay(&self, _session_id: &str) -> super::HistoryReplayFuture<'_> {
            Box::pin(async { Ok(()) })
        }

        fn publish(&self, _session_id: &str, _event: SessionEvent) -> Result<(), EventSinkClosed> {
            Ok(())
        }
    }

    /// The replay reset must be awaited before an ACP session/load request.
    /// Oracle: specification R13 and program-design AgentSessionClient.attach.
    #[tokio::test]
    async fn history_replay_contract_is_awaitable() {
        let sink: &dyn SessionEventSink = &ReplaySink;
        assert!(sink.begin_history_replay("fixture-session").await.is_ok());
    }
}
