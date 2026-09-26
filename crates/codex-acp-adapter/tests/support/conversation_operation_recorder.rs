use codex_acp_adapter::{ConversationOperationRecorder, ConversationRecordFuture};
use collaboration_protocol::{CodexGeneration, OperationId, SessionId};

pub struct AcceptingConversationRecorder;

impl ConversationOperationRecorder for AcceptingConversationRecorder {
    fn admit_create<'a>(
        &'a self,
        _: &'a OperationId,
        _: &'a CodexGeneration,
    ) -> ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn before_native_dispatch<'a>(&'a self, _: &'a OperationId) -> ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn record_created<'a>(
        &'a self,
        _: &'a OperationId,
        _: &'a SessionId,
    ) -> ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn record_failure<'a>(&'a self, _: &'a OperationId, _: bool) -> ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}
