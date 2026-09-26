//! Persistence boundary for a Codex ACP create whose caller may detach.
use collaboration_protocol::{CodexGeneration, OperationId, SessionId};
use std::{future::Future, io, pin::Pin};

pub type ConversationRecordFuture<'a> = Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;

/// The owner of operation storage implements this interface. The adapter calls
/// it before native creation I/O and after the native result is known.
pub trait ConversationOperationRecorder: Send + Sync {
    fn admit_create<'a>(
        &'a self,
        operation_id: &'a OperationId,
        generation: &'a CodexGeneration,
    ) -> ConversationRecordFuture<'a>;
    fn before_native_dispatch<'a>(
        &'a self,
        operation_id: &'a OperationId,
    ) -> ConversationRecordFuture<'a>;
    fn record_created<'a>(
        &'a self,
        operation_id: &'a OperationId,
        session_id: &'a SessionId,
    ) -> ConversationRecordFuture<'a>;
    fn record_failure<'a>(
        &'a self,
        operation_id: &'a OperationId,
        known_not_submitted: bool,
    ) -> ConversationRecordFuture<'a>;
}
