//! Host-owned admission and settlement for external ACP provider operations.

mod provider_operation_failure;
mod provider_prompt_contents;
pub use provider_prompt_contents::ProviderPromptContentsRequest;
mod provider_session_inspection;
mod provider_session_lifecycle;
mod provider_settings_control;
use provider_operation_failure::{
    admission_failure, invalid_setting_failure, prompt_runtime_failure, runtime_failure,
};

use crate::provider_operation_settlement::{
    ProviderOperationCompletion, effective_settings, optional_message_text,
    provider_applied_setting, provider_failed_setting, provider_session_record,
};
use crate::provider_queue_operation_registry::ProviderQueueOperationRegistry;
use crate::{ExternalProviderRuntime, ExternalProviderRuntimeError};
use collaboration_protocol::{
    ConversationAdmissionState, ConversationBindingIdentity, ConversationCancelRequest,
    ConversationCloseRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationFailureStage, ConversationOperationReconcileRequest,
    ConversationOperationSettlement, ConversationOperationShowRequest,
    ConversationOperationSnapshot, ConversationOperationSubmission,
    ConversationOperationWaitOutput, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationOutputUnavailableReason,
    ConversationPromptRequest, ConversationResumeRequest, EndpointRef, NonEmptyText, OperationId,
    ProviderBindingIdentity, ProviderIdentity, ProviderOperationEffect, ProviderOperationKind,
    ProviderOperationStage, ProviderPromptStopReason, ProviderReconciliationState,
    ProviderSessionInspectRequest, ProviderSettingsAcceptRequest, ProviderSettingsSetRequest,
    SessionId, SessionRef,
};
use collaboration_service::{
    ProviderConversationBackend, ProviderConversationFuture, ProviderOperationAdmission,
    ProviderOperationAdmissionResult, ProviderOperationRecord, ProviderOperationStore,
    ProviderSessionInspectFuture, ProviderSettingsFuture,
    conversation_operation_snapshot as snapshot_from_record,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::sync::{Mutex, Notify, mpsc, watch};

const MAX_RETAINED_SETTLEMENTS: usize = 256;

pub struct ExternalProviderBinding {
    pub identity: ProviderBindingIdentity,
    pub runtime: ExternalProviderRuntime,
}

#[derive(Clone)]
pub struct ExternalProviderSupervisor {
    inner: Arc<SupervisorInner>,
}

struct SupervisorInner {
    bindings: HashMap<EndpointRef, RuntimeBinding>,
    store: Arc<Mutex<ProviderOperationStore>>,
    live_operations: StdMutex<LiveOperations>,
    queued_operations: ProviderQueueOperationRegistry,
    hub: Option<Arc<collaboration_service::ProviderSessionEventHub>>,
    history_unavailable: Mutex<HashSet<SessionRef>>,
    settings_catalogs: Arc<Mutex<HashMap<SessionRef, acp_client_runtime::ProviderSettingsCatalog>>>,
    display_names: std::sync::OnceLock<collaboration_service::SessionDisplayNameCache>,
    model_catalogs:
        HashMap<EndpointRef, watch::Sender<Vec<collaboration_service::ProviderModelEntry>>>,
    catalog_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    started_at_ms: i64,
    #[cfg(test)]
    admission_test_pause: StdMutex<Option<AdmissionTestPause>>,
}

#[cfg(test)]
struct AdmissionTestPause {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

struct RuntimeBinding {
    identity: ProviderBindingIdentity,
    runtime: Arc<ExternalProviderRuntime>,
}

#[derive(Default)]
struct LiveOperations {
    operations: HashMap<OperationId, Arc<LiveOperation>>,
    terminal_order: VecDeque<OperationId>,
}

struct LiveOperation {
    result: Mutex<Option<Result<ConversationOperationSettlement, ConversationOperationFailure>>>,
    changed: Notify,
}

impl LiveOperation {
    fn pending() -> Self {
        Self {
            result: Mutex::new(None),
            changed: Notify::new(),
        }
    }
}

impl ExternalProviderSupervisor {
    pub(crate) fn queued_operation_registry(&self) -> &ProviderQueueOperationRegistry {
        &self.inner.queued_operations
    }

    pub(crate) fn runtime_for(
        &self,
        endpoint: &EndpointRef,
    ) -> Option<Arc<ExternalProviderRuntime>> {
        self.inner
            .bindings
            .get(endpoint)
            .map(|entry| Arc::clone(&entry.runtime))
    }

    #[must_use]
    pub fn provider_model_catalog(
        &self,
        endpoint: &EndpointRef,
    ) -> Option<watch::Receiver<Vec<collaboration_service::ProviderModelEntry>>> {
        self.inner
            .model_catalogs
            .get(endpoint)
            .map(watch::Sender::subscribe)
    }

    #[must_use]
    pub fn live_operation_ids(&self) -> std::collections::HashSet<OperationId> {
        self.inner
            .live_operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .operations
            .iter()
            .filter_map(|(operation_id, operation)| {
                operation.result.try_lock().map_or_else(
                    |_| Some(operation_id.clone()),
                    |result| result.is_none().then(|| operation_id.clone()),
                )
            })
            .collect()
    }
    pub fn new(
        bindings: Vec<ExternalProviderBinding>,
        store: Arc<Mutex<ProviderOperationStore>>,
    ) -> Result<Self, &'static str> {
        Self::new_with_hub(bindings, store, None)
    }

    pub(crate) fn new_with_hub(
        bindings: Vec<ExternalProviderBinding>,
        store: Arc<Mutex<ProviderOperationStore>>,
        hub: Option<Arc<collaboration_service::ProviderSessionEventHub>>,
    ) -> Result<Self, &'static str> {
        let mut runtimes = HashMap::with_capacity(bindings.len());
        let mut model_catalogs = HashMap::with_capacity(bindings.len());
        let mut catalog_tasks = Vec::with_capacity(bindings.len());
        let settings_catalogs = Arc::new(Mutex::new(HashMap::new()));
        for binding in bindings {
            let endpoint = binding.identity.endpoint.clone();
            if runtimes.contains_key(&endpoint) {
                return Err("external provider endpoint binding must be unique");
            }
            let runtime = Arc::new(binding.runtime);
            let (catalog_sender, _) = watch::channel(vec![
                crate::provider_model_catalog::provider_default_model()?,
            ]);
            let (refresh_sender, mut refresh_receiver) = mpsc::unbounded_channel::<String>();
            runtime.install_catalog_refresh(refresh_sender);
            let task_runtime = Arc::clone(&runtime);
            let task_catalogs = Arc::clone(&settings_catalogs);
            let task_endpoint = endpoint.clone();
            let task_sender = catalog_sender.clone();
            catalog_tasks.push(tokio::spawn(async move {
                while let Some(session_id) = refresh_receiver.recv().await {
                    let Some(catalog) = task_runtime.settings_catalog(&session_id).await else {
                        continue;
                    };
                    let Ok(session_id) = SessionId::try_from(session_id) else {
                        continue;
                    };
                    let target = SessionRef {
                        endpoint: task_endpoint.clone(),
                        session_id,
                    };
                    task_catalogs.lock().await.insert(target, catalog.clone());
                    task_sender
                        .send_replace(crate::provider_model_catalog::model_entries(&catalog));
                }
            }));
            model_catalogs.insert(endpoint.clone(), catalog_sender);
            runtimes.insert(
                endpoint,
                RuntimeBinding {
                    identity: binding.identity,
                    runtime,
                },
            );
        }
        Ok(Self {
            inner: Arc::new(SupervisorInner {
                bindings: runtimes,
                store,
                live_operations: StdMutex::new(LiveOperations::default()),
                queued_operations: ProviderQueueOperationRegistry::default(),
                hub,
                history_unavailable: Mutex::new(HashSet::new()),
                settings_catalogs,
                display_names: std::sync::OnceLock::new(),
                model_catalogs,
                catalog_tasks: Mutex::new(catalog_tasks),
                started_at_ms: now_ms(),
                #[cfg(test)]
                admission_test_pause: StdMutex::new(None),
            }),
        })
    }

    pub async fn install_approval_broker(
        &self,
        broker: Arc<collaboration_service::ServiceInteractionBroker>,
    ) {
        for binding in self.inner.bindings.values() {
            binding
                .runtime
                .install_approval_broker(Arc::clone(&broker))
                .await;
        }
    }

    pub(crate) fn install_display_names(
        &self,
        display_names: collaboration_service::SessionDisplayNameCache,
    ) {
        let _already_installed = self.inner.display_names.set(display_names);
    }

    #[allow(clippy::result_large_err)]
    fn runtime_binding(
        &self,
        endpoint: &EndpointRef,
        operation_id: &OperationId,
        target: Option<SessionRef>,
    ) -> Result<(ProviderBindingIdentity, Arc<ExternalProviderRuntime>), ConversationOperationFailure>
    {
        let binding = self.inner.bindings.get(endpoint).ok_or_else(|| {
            failure(
                ConversationOperationFailureKind::NotFound,
                ConversationOperationFailureStage::Binding,
                ProviderOperationEffect::None,
                "external provider binding was not found",
                operation_id.clone(),
                target.clone(),
            )
        })?;
        if binding.runtime.retirement().is_cancelled() {
            return Err(failure(
                ConversationOperationFailureKind::Unavailable,
                ConversationOperationFailureStage::Binding,
                ProviderOperationEffect::None,
                "external provider binding is retired",
                operation_id.clone(),
                target,
            ));
        }
        Ok((binding.identity.clone(), Arc::clone(&binding.runtime)))
    }

    #[allow(clippy::result_large_err)]
    async fn prepare_operation(
        &self,
        operation_id: OperationId,
        operation_kind: ProviderOperationKind,
        binding: ProviderBindingIdentity,
        known_target: Option<&SessionRef>,
    ) -> Result<PreparedOperation, ConversationOperationFailure> {
        let admitted_at_ms = now_ms();
        let failure_target = known_target.cloned();
        let mut store = self.inner.store.lock().await;
        #[cfg(test)]
        let admission_test_pause = {
            self.inner
                .admission_test_pause
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        };
        #[cfg(test)]
        if let Some(pause) = admission_test_pause {
            let _result = pause.entered.send(());
            let _result = pause.release.await;
        }
        if let Some(record) = store.inspect(&operation_id).await.map_err(|_| {
            admission_failure(
                operation_id.clone(),
                "provider operation admission could not be inspected",
                failure_target.clone(),
            )
        })? {
            return Ok(PreparedOperation::Existing(
                snapshot_from_record(record)
                    .map_err(|message| snapshot_failure(operation_id, message))?,
            ));
        }
        if operation_kind != ProviderOperationKind::ConversationCancel
            && store
                .has_blocking_uncertainty(&binding, known_target)
                .await
                .map_err(|_| unavailable_failure(operation_id.clone(), failure_target.clone()))?
        {
            return Err(failure(
                ConversationOperationFailureKind::Busy,
                ConversationOperationFailureStage::Validation,
                ProviderOperationEffect::None,
                "provider binding has an unresolved operation",
                operation_id,
                failure_target,
            ));
        }
        let admission = store
            .admit(ProviderOperationAdmission {
                operation_id: operation_id.clone(),
                operation_kind,
                binding: ConversationBindingIdentity::ExternalProvider { binding },
                admitted_at_ms,
            })
            .await
            .map_err(|_| {
                admission_failure(
                    operation_id.clone(),
                    "provider operation admission could not be persisted",
                    failure_target.clone(),
                )
            })?;
        match admission {
            ProviderOperationAdmissionResult::Existing(record) => Ok(PreparedOperation::Existing(
                snapshot_from_record(record)
                    .map_err(|message| snapshot_failure(operation_id, message))?,
            )),
            ProviderOperationAdmissionResult::Admitted(_) => {
                if let Some(target) = known_target {
                    store
                        .record_target(&operation_id, target, now_ms())
                        .await
                        .map_err(|_| {
                            admission_failure(
                                operation_id.clone(),
                                "provider operation target could not be persisted",
                                failure_target.clone(),
                            )
                        })?;
                }
                let record = store
                    .mark_may_have_dispatched(&operation_id, now_ms())
                    .await
                    .map_err(|_| {
                        admission_failure(
                            operation_id.clone(),
                            "provider dispatch marker could not be persisted",
                            failure_target,
                        )
                    })?;
                let snapshot = snapshot_from_record(record)
                    .map_err(|message| snapshot_failure(operation_id.clone(), message))?;
                let live = Arc::new(LiveOperation::pending());
                self.inner
                    .live_operations
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .operations
                    .insert(operation_id, Arc::clone(&live));
                Ok(PreparedOperation::Admitted { snapshot, live })
            }
        }
    }

    fn spawn_operation<F>(&self, operation_id: OperationId, live: Arc<LiveOperation>, operation: F)
    where
        F: std::future::Future<Output = ProviderOperationCompletion> + Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            let completion = operation.await;
            inner.finish(operation_id, live, completion).await;
        });
    }

    #[allow(clippy::result_large_err)]
    async fn inspect_record(
        &self,
        operation_id: &OperationId,
    ) -> Result<ProviderOperationRecord, ConversationOperationFailure> {
        self.inner
            .store
            .lock()
            .await
            .inspect(operation_id)
            .await
            .map_err(|_| unavailable_failure(operation_id.clone(), None))?
            .ok_or_else(|| {
                failure(
                    ConversationOperationFailureKind::NotFound,
                    ConversationOperationFailureStage::Settlement,
                    ProviderOperationEffect::None,
                    "provider operation was not found",
                    operation_id.clone(),
                    None,
                )
            })
    }

    pub async fn shutdown(&self) -> Result<(), &'static str> {
        for binding in self.inner.bindings.values() {
            binding.runtime.retirement().cancel();
        }
        for binding in self.inner.bindings.values() {
            tokio::time::timeout(Duration::from_secs(10), binding.runtime.shutdown())
                .await
                .map_err(|_| "external provider runtime shutdown timed out")?;
            if binding.runtime.shutdown_failed() {
                return Err("external provider runtime owner failed to join");
            }
        }
        for task in self.inner.catalog_tasks.lock().await.drain(..) {
            tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .map_err(|_| "provider model catalog task drain timed out")?
                .map_err(|_| "provider model catalog task failed")?;
        }
        let operations = self
            .inner
            .live_operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .operations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for operation in operations {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let changed = operation.changed.notified();
                    if operation.result.lock().await.is_some() {
                        break;
                    }
                    changed.await;
                }
            })
            .await
            .map_err(|_| "external provider operation drain timed out")?;
        }
        Ok(())
    }
}

enum PreparedOperation {
    Existing(ConversationOperationSnapshot),
    Admitted {
        snapshot: ConversationOperationSnapshot,
        live: Arc<LiveOperation>,
    },
}

impl SupervisorInner {
    async fn record_settings_catalog(
        &self,
        target: SessionRef,
        catalog: acp_client_runtime::ProviderSettingsCatalog,
    ) {
        if let Some(sender) = self.model_catalogs.get(&target.endpoint) {
            sender.send_replace(crate::provider_model_catalog::model_entries(&catalog));
        }
        self.settings_catalogs.lock().await.insert(target, catalog);
    }

    async fn finish(
        &self,
        operation_id: OperationId,
        live: Arc<LiveOperation>,
        completion: ProviderOperationCompletion,
    ) {
        let result = match completion {
            ProviderOperationCompletion::Success {
                settlement,
                target,
                session_record,
            } => {
                let terminal_stop_reason = match &settlement {
                    // The durable summary column is an old closed string enum.
                    // Keep a future value in the live settlement without writing
                    // an unreadable value into rollback-visible storage.
                    ConversationOperationSettlement::PromptCompleted {
                        stop_reason: ProviderPromptStopReason::Unknown(_),
                        ..
                    } => None,
                    ConversationOperationSettlement::PromptCompleted { stop_reason, .. } => {
                        Some(stop_reason.clone())
                    }
                    _ => None,
                };
                let mut store = self.store.lock().await;
                if crate::provider_operation_settlement::persist_provider_success(
                    &mut store,
                    &operation_id,
                    target.as_ref(),
                    session_record.as_deref(),
                    terminal_stop_reason,
                )
                .await
                .is_err()
                {
                    Err(applied_storage_failure(operation_id.clone(), target))
                } else {
                    Ok(settlement)
                }
            }
            ProviderOperationCompletion::Failure(operation_failure) => {
                let reconciliation = if operation_failure.effect == ProviderOperationEffect::Unknown
                {
                    ProviderReconciliationState::Unresolved
                } else {
                    ProviderReconciliationState::Confirmed
                };
                let mut store = self.store.lock().await;
                let target = operation_failure.target.clone();
                let target_record_failed =
                    if operation_failure.effect == ProviderOperationEffect::Applied {
                        match target.as_ref() {
                            Some(target) => store
                                .record_target(&operation_id, target, now_ms())
                                .await
                                .is_err(),
                            None => false,
                        }
                    } else {
                        false
                    };
                if target_record_failed {
                    Err(applied_storage_failure(operation_id.clone(), target))
                } else {
                    let _ = store
                        .record_terminal(
                            &operation_id,
                            operation_failure.effect,
                            reconciliation,
                            None,
                            now_ms(),
                        )
                        .await;
                    Err(operation_failure)
                }
            }
            ProviderOperationCompletion::FailureWithSession {
                failure,
                session_record,
            } => {
                let mut store = self.store.lock().await;
                if store
                    .settle_session_operation(&operation_id, &session_record)
                    .await
                    .is_err()
                {
                    Err(applied_storage_failure(
                        operation_id.clone(),
                        Some(session_record.target.clone()),
                    ))
                } else {
                    Err(failure)
                }
            }
        };
        *live.result.lock().await = Some(result);
        live.changed.notify_waiters();
        let mut operations = self
            .live_operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        operations.terminal_order.push_back(operation_id);
        while operations.terminal_order.len() > MAX_RETAINED_SETTLEMENTS {
            if let Some(expired) = operations.terminal_order.pop_front() {
                operations.operations.remove(&expired);
            }
        }
    }
}

#[allow(clippy::result_large_err)]
fn retain_operation<T, F>(
    operation_id: OperationId,
    target: Option<SessionRef>,
    operation: F,
) -> ProviderConversationFuture<'static, T>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, ConversationOperationFailure>> + Send + 'static,
{
    let (reply, result) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = reply.send(operation.await);
    });
    Box::pin(async move {
        result.await.unwrap_or_else(|_| {
            Err(failure(
                ConversationOperationFailureKind::Unavailable,
                ConversationOperationFailureStage::Settlement,
                ProviderOperationEffect::Unknown,
                "provider operation owner stopped before returning admission",
                operation_id,
                target,
            ))
        })
    })
}

impl ProviderConversationBackend for ExternalProviderSupervisor {
    fn resume(
        &self,
        request: ConversationResumeRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        provider_session_lifecycle::resume(self, request)
    }

    fn close(
        &self,
        request: ConversationCloseRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        provider_session_lifecycle::close(self, request)
    }

    fn inspect_session(
        &self,
        request: ProviderSessionInspectRequest,
    ) -> ProviderSessionInspectFuture<'_> {
        Box::pin(provider_session_inspection::inspect(self, request))
    }

    fn settings_set(&self, request: ProviderSettingsSetRequest) -> ProviderSettingsFuture<'_> {
        Box::pin(provider_settings_control::set(self, request))
    }

    fn settings_accept(
        &self,
        request: ProviderSettingsAcceptRequest,
    ) -> ProviderSettingsFuture<'_> {
        Box::pin(provider_settings_control::accept(self, request))
    }

    fn binding(&self, endpoint: &EndpointRef) -> Option<ProviderBindingIdentity> {
        self.inner
            .bindings
            .get(endpoint)
            .map(|binding| binding.identity.clone())
    }

    fn lookup_existing(
        &self,
        operation_id: OperationId,
    ) -> ProviderConversationFuture<'_, Option<ConversationOperationSnapshot>> {
        Box::pin(async move {
            self.inner
                .store
                .lock()
                .await
                .inspect(&operation_id)
                .await
                .map_err(|_| unavailable_failure(operation_id.clone(), None))?
                .map(snapshot_from_record)
                .transpose()
                .map_err(|message| snapshot_failure(operation_id, message))
        })
    }

    fn create(
        &self,
        request: ConversationCreateRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        let operation_id = request.operation_id.clone();
        let backend = self.clone();
        retain_operation(operation_id, None, async move {
            let (binding, runtime) =
                backend.runtime_binding(&request.endpoint, &request.operation_id, None)?;
            let prepared = backend
                .prepare_operation(
                    request.operation_id.clone(),
                    ProviderOperationKind::ConversationCreate,
                    binding.clone(),
                    None,
                )
                .await?;
            match prepared {
                PreparedOperation::Existing(operation) => Ok(existing_submission(operation)),
                PreparedOperation::Admitted { snapshot, live } => {
                    let operation_id = request.operation_id.clone();
                    let requested_policy = request.requested_policy;
                    let endpoint = binding.endpoint;
                    let working_directory = request.working_directory;
                    let created_by = request.created_by;
                    let approver = request.approver;
                    let requested_settings = request.settings.unwrap_or_default();
                    let completion_inner = Arc::clone(&backend.inner);
                    backend.spawn_operation(operation_id.clone(), live, async move {
                        match runtime
                            .create_session_with_settings(
                                PathBuf::from(String::from(working_directory.clone())),
                                acp_client_runtime::RequestedProviderSettings {
                                    mode: requested_settings.mode,
                                    model: requested_settings.model,
                                    effort: requested_settings.effort,
                                },
                            )
                            .await
                        {
                            Ok(created) => {
                                let catalog = runtime.settings_catalog(&created.provider_session_id).await;
                                match SessionId::try_from(created.provider_session_id) {
                                    Ok(session_id) => {
                                        let target = SessionRef {
                                            endpoint,
                                            session_id,
                                        };
                                        if let Some(catalog) = catalog {
                                            completion_inner.record_settings_catalog(target.clone(), catalog).await;
                                        }
                                        let mut observed = effective_settings(requested_policy.clone());
                                        observed.mode = created.effective_settings.mode;
                                        observed.model = created.effective_settings.model;
                                        observed.effort = created.effective_settings.effort;
                                        ProviderOperationCompletion::Success {
                                            settlement: ConversationOperationSettlement::Created {
                                                target: target.clone(),
                                                effective_settings: observed,
                                            },
                                            target: Some(target.clone()),
                                            session_record: Some(Box::new(
                                                provider_session_record(
                                                    target,
                                                    working_directory,
                                                    requested_policy,
                                                    created_by,
                                                    approver,
                                                ),
                                            )),
                                        }
                                    }
                                    Err(_) => ProviderOperationCompletion::Failure(failure(
                                        ConversationOperationFailureKind::ProtocolViolation,
                                        ConversationOperationFailureStage::Settlement,
                                        ProviderOperationEffect::Applied,
                                        "provider returned an invalid conversation identity",
                                        operation_id,
                                        None,
                                    )),
                                }
                            }
                            Err(ExternalProviderRuntimeError::CreatedWithoutSettings {
                                provider_session_id,
                                applied,
                                failed,
                                not_applied,
                            }) => {
                                let catalog = runtime.settings_catalog(&provider_session_id).await;
                                match SessionId::try_from(provider_session_id) {
                                Ok(session_id) => {
                                    let target = SessionRef { endpoint, session_id };
                                    if let Some(catalog) = catalog {
                                        completion_inner.record_settings_catalog(target.clone(), catalog).await;
                                    }
                                    ProviderOperationCompletion::Success {
                                        settlement: ConversationOperationSettlement::CreatedWithoutSettings {
                                            target: target.clone(),
                                            applied: applied.into_iter().map(provider_applied_setting).collect(),
                                            failed: vec![provider_failed_setting(failed)],
                                            not_applied: not_applied.into_iter().map(crate::provider_operation_settlement::provider_not_applied_setting).collect(),
                                        },
                                        target: Some(target.clone()),
                                        session_record: Some(Box::new(provider_session_record(
                                            target,
                                            working_directory,
                                            requested_policy,
                                            created_by,
                                            approver,
                                        ))),
                                    }
                                }
                                Err(_) => ProviderOperationCompletion::Failure(failure(
                                    ConversationOperationFailureKind::ProtocolViolation,
                                    ConversationOperationFailureStage::Settlement,
                                    ProviderOperationEffect::Applied,
                                    "provider returned an invalid conversation identity after partial settings",
                                    operation_id,
                                    None,
                                )),
                                }
                            }
                            Err(ExternalProviderRuntimeError::InvalidSetting {
                                setting,
                                value,
                                advertised,
                                provider_session_id,
                                disposition,
                            }) => {
                                let target = SessionId::try_from(provider_session_id)
                                    .ok()
                                    .map(|session_id| SessionRef { endpoint, session_id });
                                let failure = invalid_setting_failure(
                                    operation_id,
                                    target.clone(),
                                    setting,
                                    value,
                                    advertised,
                                    disposition,
                                );
                                match (target, disposition) {
                                    (Some(target), acp_client_runtime::InvalidSettingSessionDisposition::RemainsCreated) => {
                                        ProviderOperationCompletion::FailureWithSession {
                                            failure,
                                            session_record: Box::new(provider_session_record(
                                                target,
                                                working_directory,
                                                requested_policy,
                                                created_by,
                                                approver,
                                            )),
                                        }
                                    }
                                    _ => ProviderOperationCompletion::Failure(failure),
                                }
                            }
                            Err(error) => ProviderOperationCompletion::Failure(runtime_failure(
                                operation_id,
                                None,
                                error,
                            )),
                        }
                    });
                    Ok(admitted_submission(snapshot))
                }
            }
        })
    }

    fn load(
        &self,
        request: ConversationLoadRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        let operation_id = request.operation_id.clone();
        let failure_target = Some(request.target.clone());
        let backend = self.clone();
        retain_operation(operation_id, failure_target, async move {
            let target = request.target.clone();
            let (binding, runtime) = backend.runtime_binding(
                &target.endpoint,
                &request.operation_id,
                Some(target.clone()),
            )?;
            if !runtime.admission().supports_load {
                return Err(failure(
                    ConversationOperationFailureKind::UnsupportedCapability,
                    ConversationOperationFailureStage::Binding,
                    ProviderOperationEffect::None,
                    "provider runtime does not support conversation load",
                    request.operation_id,
                    Some(target),
                ));
            }
            let prepared = backend
                .prepare_operation(
                    request.operation_id.clone(),
                    ProviderOperationKind::ConversationLoad,
                    binding,
                    Some(&target),
                )
                .await?;
            match prepared {
                PreparedOperation::Existing(operation) => Ok(existing_submission(operation)),
                PreparedOperation::Admitted { snapshot, live } => {
                    let operation_id = request.operation_id.clone();
                    let provider_session_id = String::from(target.session_id.clone());
                    let requested_policy = request.requested_policy;
                    let working_directory = request.working_directory;
                    let created_by = request.requested_by;
                    let approver = request.approver;
                    let completion_target = target.clone();
                    let completion_inner = Arc::clone(&backend.inner);
                    backend.spawn_operation(operation_id.clone(), live, async move {
                        match runtime
                            .load_session(
                                provider_session_id.clone(),
                                PathBuf::from(String::from(working_directory.clone())),
                            )
                            .await
                        {
                            Ok(()) => {
                                let mut observed = effective_settings(requested_policy.clone());
                                if let Some(catalog) =
                                    runtime.settings_catalog(&provider_session_id).await
                                {
                                    let effective = catalog.effective_settings();
                                    observed.mode = effective.mode;
                                    observed.model = effective.model;
                                    observed.effort = effective.effort;
                                    completion_inner
                                        .record_settings_catalog(completion_target.clone(), catalog)
                                        .await;
                                }
                                completion_inner
                                    .history_unavailable
                                    .lock()
                                    .await
                                    .remove(&completion_target);
                                ProviderOperationCompletion::Success {
                                    settlement: ConversationOperationSettlement::Loaded {
                                        target: completion_target.clone(),
                                        effective_settings: observed,
                                    },
                                    target: Some(completion_target.clone()),
                                    session_record: Some(Box::new(provider_session_record(
                                        completion_target,
                                        working_directory,
                                        requested_policy,
                                        created_by,
                                        approver,
                                    ))),
                                }
                            }
                            Err(error) => ProviderOperationCompletion::Failure(runtime_failure(
                                operation_id,
                                Some(completion_target),
                                error,
                            )),
                        }
                    });
                    Ok(admitted_submission(snapshot))
                }
            }
        })
    }

    fn prompt(
        &self,
        request: ConversationPromptRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        let operation_id = request.operation_id.clone();
        let target = Some(request.target.clone());
        match ProviderPromptContentsRequest::from_control(request) {
            Ok(contents_request) => self.prompt_contents(contents_request),
            Err(error) => retain_operation(operation_id, target, async move { Err(*error) }),
        }
    }

    fn cancel(
        &self,
        request: ConversationCancelRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        let operation_id = request.operation_id.clone();
        let failure_target = Some(request.target.clone());
        let backend = self.clone();
        retain_operation(operation_id, failure_target, async move {
            let target = request.target.clone();
            let (binding, runtime) = backend.runtime_binding(
                &target.endpoint,
                &request.operation_id,
                Some(target.clone()),
            )?;
            if let Some(record) = backend
                .inner
                .store
                .lock()
                .await
                .inspect(&request.operation_id)
                .await
                .map_err(|_| {
                    unavailable_failure(request.operation_id.clone(), Some(target.clone()))
                })?
            {
                return Ok(existing_submission(snapshot_from_record(record).map_err(
                    |message| snapshot_failure(request.operation_id, message),
                )?));
            }
            let target_operation = backend.inspect_record(&request.target_operation_id).await?;
            if target_operation.operation_kind != ProviderOperationKind::ConversationPrompt
                || target_operation.target.as_ref() != Some(&target)
                || !matches!(
                    &target_operation.binding,
                    ConversationBindingIdentity::ExternalProvider { binding }
                        if request.generation.as_ref() == Some(&binding.generation)
                )
                || target_operation.stage == ProviderOperationStage::Terminal
            {
                return Err(failure(
                    ConversationOperationFailureKind::NotFound,
                    ConversationOperationFailureStage::Validation,
                    ProviderOperationEffect::None,
                    "target provider prompt operation is not active on this binding",
                    request.operation_id,
                    Some(target),
                ));
            }
            let prepared = backend
                .prepare_operation(
                    request.operation_id.clone(),
                    ProviderOperationKind::ConversationCancel,
                    binding,
                    Some(&target),
                )
                .await?;
            match prepared {
                PreparedOperation::Existing(operation) => Ok(existing_submission(operation)),
                PreparedOperation::Admitted { snapshot, live } => {
                    let operation_id = request.operation_id.clone();
                    let target_operation_id = request.target_operation_id;
                    let provider_session_id = String::from(target.session_id.clone());
                    let completion_target = target.clone();
                    backend.spawn_operation(operation_id.clone(), live, async move {
                        match runtime
                            .cancel_prompt_operation(
                                provider_session_id,
                                target_operation_id.clone(),
                            )
                            .await
                        {
                            Ok(()) => ProviderOperationCompletion::Success {
                                settlement: ConversationOperationSettlement::CancelRequested {
                                    target: completion_target.clone(),
                                    target_operation_id,
                                },
                                target: Some(completion_target),
                                session_record: None,
                            },
                            Err(error) => ProviderOperationCompletion::Failure(runtime_failure(
                                operation_id,
                                Some(completion_target),
                                error,
                            )),
                        }
                    });
                    Ok(admitted_submission(snapshot))
                }
            }
        })
    }

    fn show(
        &self,
        request: ConversationOperationShowRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
        Box::pin(async move {
            let stored_record = self
                .inner
                .store
                .lock()
                .await
                .inspect(&request.operation_id)
                .await
                .map_err(|_| unavailable_failure(request.operation_id.clone(), None))?;
            if let Some(record) = stored_record {
                return snapshot_from_record(record)
                    .map_err(|message| snapshot_failure(request.operation_id, message));
            }
            if let Some(snapshot) = self
                .inner
                .queued_operations
                .snapshot(&request.operation_id)
                .map_err(|message| snapshot_failure(request.operation_id.clone(), message))?
            {
                return Ok(snapshot);
            }
            Err(failure(
                ConversationOperationFailureKind::NotFound,
                ConversationOperationFailureStage::Settlement,
                ProviderOperationEffect::None,
                "provider operation was not found",
                request.operation_id,
                None,
            ))
        })
    }

    fn wait(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationWaitResult> {
        Box::pin(async move {
            let timeout_seconds = u32::from(request.timeout_seconds);
            let operation_id = request.operation_id;
            let live = self
                .inner
                .live_operations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .operations
                .get(&operation_id)
                .cloned();
            if let Some(live) = live {
                let deadline =
                    tokio::time::Instant::now() + Duration::from_secs(timeout_seconds.into());
                loop {
                    let changed = live.changed.notified();
                    if let Some(result) = live.result.lock().await.clone() {
                        let operation =
                            snapshot_from_record(self.inspect_record(&operation_id).await?)
                                .map_err(|message| {
                                    snapshot_failure(operation_id.clone(), message)
                                })?;
                        return result.map(|settlement| ConversationOperationWaitResult {
                            operation,
                            output: ConversationOperationWaitOutput::Available { settlement },
                        });
                    }
                    if tokio::time::timeout_at(deadline, changed).await.is_err() {
                        let operation =
                            snapshot_from_record(self.inspect_record(&operation_id).await?)
                                .map_err(|message| {
                                    snapshot_failure(operation_id.clone(), message)
                                })?;
                        return Ok(ConversationOperationWaitResult {
                            operation,
                            output: ConversationOperationWaitOutput::Pending,
                        });
                    }
                }
            }

            let record = self.inspect_record(&operation_id).await?;
            let output = if record.stage == ProviderOperationStage::Terminal {
                ConversationOperationWaitOutput::OutputUnavailable {
                    reason: if record.admitted_at_ms <= self.inner.started_at_ms {
                        ConversationOutputUnavailableReason::HostRestarted
                    } else {
                        ConversationOutputUnavailableReason::NotRetained
                    },
                }
            } else {
                ConversationOperationWaitOutput::Pending
            };
            let operation = snapshot_from_record(record)
                .map_err(|message| snapshot_failure(operation_id, message))?;
            Ok(ConversationOperationWaitResult { operation, output })
        })
    }

    fn reconcile(
        &self,
        request: ConversationOperationReconcileRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
        Box::pin(async move {
            // Serialize against prepare_operation, which holds this same store
            // lock until its live owner is inserted. Store -> live is the
            // single lock order for admission and reconciliation.
            let mut store = self.inner.store.lock().await;
            let is_live = self
                .inner
                .live_operations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .operations
                .get(&request.operation_id)
                .map(|operation| match operation.result.try_lock() {
                    Ok(result) => result.is_none(),
                    Err(_) => true,
                })
                .unwrap_or(false);
            if is_live {
                let record = store
                    .inspect(&request.operation_id)
                    .await
                    .map_err(|_| unavailable_failure(request.operation_id.clone(), None))?
                    .ok_or_else(|| {
                        failure(
                            ConversationOperationFailureKind::NotFound,
                            ConversationOperationFailureStage::Settlement,
                            ProviderOperationEffect::None,
                            "provider operation was not found",
                            request.operation_id.clone(),
                            None,
                        )
                    })?;
                return snapshot_from_record(record)
                    .map_err(|message| snapshot_failure(request.operation_id, message));
            }
            let record = store
                .inspect(&request.operation_id)
                .await
                .map_err(|_| unavailable_failure(request.operation_id.clone(), None))?
                .ok_or_else(|| {
                    failure(
                        ConversationOperationFailureKind::NotFound,
                        ConversationOperationFailureStage::Settlement,
                        ProviderOperationEffect::None,
                        "provider operation was not found",
                        request.operation_id.clone(),
                        None,
                    )
                })?;
            let record = if record.reconciliation_state == ProviderReconciliationState::Unresolved {
                store
                    .record_not_reconcilable(&request.operation_id, now_ms())
                    .await
                    .map_err(|_| unavailable_failure(request.operation_id.clone(), record.target))?
            } else {
                record
            };
            snapshot_from_record(record).map_err(|message| {
                failure(
                    ConversationOperationFailureKind::ProtocolViolation,
                    ConversationOperationFailureStage::Reconciliation,
                    ProviderOperationEffect::Unknown,
                    message,
                    request.operation_id,
                    None,
                )
            })
        })
    }
}

fn existing_submission(
    operation: ConversationOperationSnapshot,
) -> ConversationOperationSubmission {
    ConversationOperationSubmission {
        admission: ConversationAdmissionState::Existing,
        operation,
    }
}

fn admitted_submission(
    operation: ConversationOperationSnapshot,
) -> ConversationOperationSubmission {
    ConversationOperationSubmission {
        admission: ConversationAdmissionState::Admitted,
        operation,
    }
}

fn unavailable_failure(
    operation_id: OperationId,
    target: Option<SessionRef>,
) -> ConversationOperationFailure {
    failure(
        ConversationOperationFailureKind::Unavailable,
        ConversationOperationFailureStage::Settlement,
        ProviderOperationEffect::None,
        "provider operation metadata is unavailable",
        operation_id,
        target,
    )
}

fn applied_storage_failure(
    operation_id: OperationId,
    target: Option<SessionRef>,
) -> ConversationOperationFailure {
    failure(
        ConversationOperationFailureKind::Unavailable,
        ConversationOperationFailureStage::Settlement,
        ProviderOperationEffect::Applied,
        "provider settlement could not be persisted after the operation applied",
        operation_id,
        target,
    )
}

fn snapshot_failure(
    operation_id: OperationId,
    message: &'static str,
) -> ConversationOperationFailure {
    failure(
        ConversationOperationFailureKind::ProtocolViolation,
        ConversationOperationFailureStage::Settlement,
        ProviderOperationEffect::Unknown,
        message,
        operation_id,
        None,
    )
}

#[allow(clippy::expect_used)]
fn failure(
    kind: ConversationOperationFailureKind,
    stage: ConversationOperationFailureStage,
    effect: ProviderOperationEffect,
    message: &'static str,
    operation_id: OperationId,
    target: Option<SessionRef>,
) -> ConversationOperationFailure {
    ConversationOperationFailure {
        kind,
        stage,
        effect,
        message: NonEmptyText::try_from(message.to_owned())
            .expect("static provider failure message is valid"),
        operation_id: Some(operation_id),
        invalid_setting: None,
        provider_code: None,
        target,
        endpoint: None,
        availability: None,
    }
}

fn failure_with_provider_code(
    kind: ConversationOperationFailureKind,
    stage: ConversationOperationFailureStage,
    effect: ProviderOperationEffect,
    message: &'static str,
    operation_id: OperationId,
    target: Option<SessionRef>,
    provider_code: Option<i64>,
) -> ConversationOperationFailure {
    let mut operation_failure = failure(kind, stage, effect, message, operation_id, target);
    operation_failure.provider_code = provider_code;
    operation_failure
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

mod provider_delivery_submission;
#[cfg(test)]
mod tests;
pub(crate) use provider_delivery_submission::ProviderPromptDispatch;
