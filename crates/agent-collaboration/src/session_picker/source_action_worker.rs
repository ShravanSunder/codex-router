//! Dropping a superseded preparation future cancels its read-only SSH process owner.
use super::source_action_state::PendingSourceAction;
use crate::sessions::{
    router_connection_registry::RouterConnectionProfile,
    source_ssh_protocol::{PreparedSourceLaunch, SourceReadError, SourceSessionAction},
};
use std::{future::Future, pin::Pin, sync::Arc};

pub(crate) type SourceActionPreparer = Arc<
    dyn Fn(
            RouterConnectionProfile,
            SourceSessionAction,
        )
            -> Pin<Box<dyn Future<Output = Result<PreparedSourceLaunch, SourceReadError>> + Send>>
        + Send
        + Sync,
>;

pub(super) fn production_preparer() -> SourceActionPreparer {
    Arc::new(|profile, action| {
        Box::pin(async move {
            crate::sessions::source_ssh_transport::prepare_configured_source(&profile, action).await
        })
    })
}

pub(super) async fn run_source_preparation(
    mut receiver: tokio::sync::watch::Receiver<Option<PendingSourceAction>>,
    prepare: SourceActionPreparer,
    mut publish: impl FnMut(PendingSourceAction, Result<PreparedSourceLaunch, SourceReadError>),
) {
    loop {
        let request = receiver.borrow_and_update().clone();
        if let Some(request) = request {
            let future = prepare(request.profile.clone(), request.action.clone());
            tokio::select! {
                biased;
                changed = receiver.changed() => { if changed.is_err() { return; } continue; }
                result = future => publish(request, result),
            }
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

type SourcePreparationReceiver = tokio::sync::watch::Receiver<Option<PendingSourceAction>>;

#[derive(Clone)]
pub(super) struct SourcePreparationPort {
    sender: tokio::sync::watch::Sender<Option<PendingSourceAction>>,
    receiver: Arc<std::sync::Mutex<Option<SourcePreparationReceiver>>>,
}

impl SourcePreparationPort {
    pub(super) fn new() -> Self {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        Self {
            sender,
            receiver: Arc::new(std::sync::Mutex::new(Some(receiver))),
        }
    }
    pub(super) fn take_receiver(&self) -> Option<SourcePreparationReceiver> {
        self.receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
    pub(super) fn synchronize(&self, pending: Option<PendingSourceAction>) {
        self.sender.send_if_modified(|current| {
            if *current == pending {
                false
            } else {
                *current = pending;
                true
            }
        });
    }
}

pub(super) fn use_source_preparation(
    hooks: &mut iocraft::prelude::Hooks<'_, '_>,
    model: iocraft::prelude::State<super::picker_model::SessionsPickerModel>,
    selected_outcome: iocraft::prelude::State<Option<super::SessionsPickerOutcome>>,
    preparer: Option<SourceActionPreparer>,
) {
    use iocraft::prelude::*;
    let port = hooks.use_memo(SourcePreparationPort::new, ());
    hooks.use_future({
        let receiver = port.take_receiver();
        let preparer = preparer.unwrap_or_else(production_preparer);
        let mut model = model;
        let mut selected_outcome = selected_outcome;
        async move {
            if let Some(receiver) = receiver {
                run_source_preparation(receiver, preparer, move |request, result| {
                    if let Some(outcome) = model.write().complete_source_action(&request, result) {
                        selected_outcome.set(Some(outcome));
                    }
                })
                .await;
            }
        }
    });
    port.synchronize(model.read().source_action.pending.clone());
}
