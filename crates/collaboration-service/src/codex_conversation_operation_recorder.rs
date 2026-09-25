//! Codex ACP create evidence in the shared conversation operation store.
use crate::{ProviderOperationAdmission, ProviderOperationAdmissionResult, ProviderOperationStore};
use codex_acp_adapter::{ConversationOperationRecorder, ConversationRecordFuture};
use collaboration_protocol::{
    CodexGeneration, ConversationBindingIdentity, EndpointRef, NonEmptyText, OperationId,
    ProviderOperationEffect, ProviderOperationKind, ProviderOperationStage,
    ProviderReconciliationState, SessionId, SessionRef,
};
use std::{
    collections::HashSet,
    io,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
pub struct CodexConversationOperationRecorder {
    store: Arc<Mutex<ProviderOperationStore>>,
    endpoint: EndpointRef,
    listener_path: NonEmptyText,
    changed: Arc<Notify>,
    active: Arc<StdMutex<HashSet<OperationId>>>,
}

impl CodexConversationOperationRecorder {
    pub fn new(
        store: Arc<Mutex<ProviderOperationStore>>,
        endpoint: EndpointRef,
        listener_path: NonEmptyText,
    ) -> Self {
        Self {
            store,
            endpoint,
            listener_path,
            changed: Arc::new(Notify::new()),
            active: Arc::new(StdMutex::new(HashSet::new())),
        }
    }

    pub fn changed(&self) -> Arc<Notify> {
        Arc::clone(&self.changed)
    }

    pub fn is_active(&self, operation_id: &OperationId) -> bool {
        self.active
            .lock()
            .is_ok_and(|active| active.contains(operation_id))
    }
}

/// Keeps non-create ACP traffic available when operation storage cannot start.
pub struct UnavailableConversationOperationRecorder;

impl ConversationOperationRecorder for UnavailableConversationOperationRecorder {
    fn admit_create<'a>(
        &'a self,
        _: &'a OperationId,
        _: &'a CodexGeneration,
    ) -> ConversationRecordFuture<'a> {
        Box::pin(async {
            Err(io::Error::other(
                "conversation operation storage unavailable",
            ))
        })
    }
    fn before_native_dispatch<'a>(&'a self, _: &'a OperationId) -> ConversationRecordFuture<'a> {
        Box::pin(async {
            Err(io::Error::other(
                "conversation operation storage unavailable",
            ))
        })
    }
    fn record_created<'a>(
        &'a self,
        _: &'a OperationId,
        _: &'a SessionId,
    ) -> ConversationRecordFuture<'a> {
        Box::pin(async {
            Err(io::Error::other(
                "conversation operation storage unavailable",
            ))
        })
    }
    fn record_failure<'a>(&'a self, _: &'a OperationId, _: bool) -> ConversationRecordFuture<'a> {
        Box::pin(async {
            Err(io::Error::other(
                "conversation operation storage unavailable",
            ))
        })
    }
}

impl ConversationOperationRecorder for CodexConversationOperationRecorder {
    fn admit_create<'a>(
        &'a self,
        operation_id: &'a OperationId,
        generation: &'a CodexGeneration,
    ) -> ConversationRecordFuture<'a> {
        Box::pin(async move {
            let result = self
                .store
                .lock()
                .await
                .admit(ProviderOperationAdmission {
                    operation_id: operation_id.clone(),
                    operation_kind: ProviderOperationKind::ConversationCreate,
                    binding: ConversationBindingIdentity::CodexAcp {
                        endpoint: self.endpoint.clone(),
                        listener_path: self.listener_path.clone(),
                        generation: generation.clone(),
                    },
                    admitted_at_ms: chrono::Utc::now().timestamp_millis(),
                })
                .await
                .map_err(io::Error::other)?;
            match result {
                ProviderOperationAdmissionResult::Admitted(_) => {
                    self.active
                        .lock()
                        .map_err(|_| {
                            io::Error::other("conversation operation tracking unavailable")
                        })?
                        .insert(operation_id.clone());
                    self.changed.notify_waiters();
                    Ok(())
                }
                ProviderOperationAdmissionResult::Existing(_) => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "conversation create operation already exists; inspect its operation ID",
                )),
            }
        })
    }

    fn before_native_dispatch<'a>(
        &'a self,
        operation_id: &'a OperationId,
    ) -> ConversationRecordFuture<'a> {
        Box::pin(async move {
            self.store
                .lock()
                .await
                .mark_may_have_dispatched(operation_id, chrono::Utc::now().timestamp_millis())
                .await
                .map_err(io::Error::other)?;
            self.changed.notify_waiters();
            Ok(())
        })
    }

    fn record_created<'a>(
        &'a self,
        operation_id: &'a OperationId,
        session_id: &'a SessionId,
    ) -> ConversationRecordFuture<'a> {
        Box::pin(async move {
            let mut store = self.store.lock().await;
            store
                .record_target(
                    operation_id,
                    &SessionRef {
                        endpoint: self.endpoint.clone(),
                        session_id: session_id.clone(),
                    },
                    chrono::Utc::now().timestamp_millis(),
                )
                .await
                .map_err(io::Error::other)?;
            self.changed.notify_waiters();
            store
                .record_terminal(
                    operation_id,
                    ProviderOperationEffect::Applied,
                    ProviderReconciliationState::Confirmed,
                    chrono::Utc::now().timestamp_millis(),
                )
                .await
                .map_err(io::Error::other)?;
            self.changed.notify_waiters();
            self.active
                .lock()
                .map_err(|_| io::Error::other("conversation operation tracking unavailable"))?
                .remove(operation_id);
            Ok(())
        })
    }

    fn record_failure<'a>(
        &'a self,
        operation_id: &'a OperationId,
        known_not_submitted: bool,
    ) -> ConversationRecordFuture<'a> {
        Box::pin(async move {
            let mut store = self.store.lock().await;
            let record = store
                .inspect(operation_id)
                .await
                .map_err(io::Error::other)?
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "conversation operation missing")
                })?;
            if record.stage == ProviderOperationStage::Terminal {
                return Ok(());
            }
            let known_none =
                known_not_submitted || record.stage == ProviderOperationStage::Admitted;
            let effect = if known_none {
                ProviderOperationEffect::None
            } else {
                ProviderOperationEffect::Unknown
            };
            let reconciliation = if known_none {
                ProviderReconciliationState::NotReconcilable
            } else {
                ProviderReconciliationState::Unresolved
            };
            store
                .record_terminal(
                    operation_id,
                    effect,
                    reconciliation,
                    chrono::Utc::now().timestamp_millis(),
                )
                .await
                .map_err(io::Error::other)?;
            self.changed.notify_waiters();
            self.active
                .lock()
                .map_err(|_| io::Error::other("conversation operation tracking unavailable"))?
                .remove(operation_id);
            Ok(())
        })
    }
}
