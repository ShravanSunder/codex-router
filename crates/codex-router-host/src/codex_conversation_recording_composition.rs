//! Host composition for durable Codex ACP conversation creation.
use collaboration_protocol::{EndpointId, EndpointRef, NonEmptyText, UuidIdentity};
use collaboration_service::{
    CodexConversationOperationRecorder, ConversationOperationRecorder, ProviderOperationStore,
    UnavailableConversationOperationRecorder,
};
use std::{io, path::Path, sync::Arc};

pub(crate) fn recording_for_store(
    store: Option<&Arc<tokio::sync::Mutex<ProviderOperationStore>>>,
    directory: &Path,
    service_id: &UuidIdentity,
) -> io::Result<Option<Arc<CodexConversationOperationRecorder>>> {
    store
        .map(|store| {
            Ok(Arc::new(CodexConversationOperationRecorder::new(
                Arc::clone(store),
                EndpointRef {
                    service_id: service_id.clone(),
                    endpoint_id: EndpointId::try_from("codex-local".to_owned())
                        .map_err(io::Error::other)?,
                },
                NonEmptyText::try_from(
                    directory
                        .join("codex-acp.sock")
                        .to_string_lossy()
                        .into_owned(),
                )
                .map_err(io::Error::other)?,
            )))
        })
        .transpose()
}

pub(crate) fn adapter_recorder(
    recorder: Option<Arc<CodexConversationOperationRecorder>>,
) -> Arc<dyn ConversationOperationRecorder> {
    recorder.map_or_else(
        || {
            Arc::new(UnavailableConversationOperationRecorder)
                as Arc<dyn ConversationOperationRecorder>
        },
        |recorder| recorder as Arc<dyn ConversationOperationRecorder>,
    )
}
