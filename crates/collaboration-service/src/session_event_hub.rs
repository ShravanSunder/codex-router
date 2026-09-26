//! Read-side contract for Router-owned provider Sessions.
//!
//! The event hub implementation belongs to the Session model slice. The
//! app-server and ACP-agent faces consume this trait without owning state.
use message_board::{SessionEndpointRef, SessionRef};
use session_event_model::{SessionEvent, SessionState};
use std::{future::Future, path::PathBuf, pin::Pin};
use tokio::sync::broadcast;

pub type HubFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, SessionEventHubError>> + Send + 'a>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubEvent {
    pub sequence: u64,
    pub event: SessionEvent,
}

pub struct SessionEventAttachment {
    pub snapshot: Vec<HubEvent>,
    pub receiver: broadcast::Receiver<HubEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubSessionSummary {
    pub session: SessionRef,
    pub working_directory: PathBuf,
    pub created_at_seconds: i64,
    pub updated_at_seconds: i64,
    pub preview: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub state: SessionState,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum SessionEventHubError {
    #[error("session not found")]
    SessionNotFound,
    #[error("session event hub unavailable")]
    Unavailable,
}

/// Attach must capture its snapshot and subscribe to later events atomically.
/// The implementation owns the event log and sequence numbers. Sessions
/// returns only Sessions on the requested provider endpoint.
pub trait SessionEventHub: Send + Sync {
    fn attach(&self, session: SessionRef) -> HubFuture<'_, SessionEventAttachment>;
    fn state(&self, session: SessionRef) -> HubFuture<'_, SessionState>;
    fn sessions(&self, endpoint: SessionEndpointRef) -> HubFuture<'_, Vec<HubSessionSummary>>;
}
