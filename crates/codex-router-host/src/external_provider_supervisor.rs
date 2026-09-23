//! Host-owned admission and settlement for external ACP provider operations.

use crate::{ExternalProviderRuntime, ExternalProviderRuntimeError};
use collaboration_protocol::{
    ConversationAdmissionState, ConversationCancelRequest, ConversationCreateRequest,
    ConversationLoadRequest, ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationFailureStage, ConversationOperationReconcileRequest,
    ConversationOperationSettlement, ConversationOperationShowRequest,
    ConversationOperationSnapshot, ConversationOperationSubmission,
    ConversationOperationWaitOutput, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationOutputUnavailableReason,
    ConversationPromptRequest, EffectiveProviderSettings, EndpointRef, MessageText, NonEmptyText,
    ObservationTimestamp, OperationId, ProviderAuthenticationState, ProviderBindingIdentity,
    ProviderOperationEffect, ProviderOperationKind, ProviderOperationStage,
    ProviderReconciliationState, ProviderSettingsMappingStatus, SessionId, SessionRef,
    render_message,
};
use collaboration_service::{
    ProviderConversationBackend, ProviderConversationFuture, ProviderOperationAdmission,
    ProviderOperationAdmissionResult, ProviderOperationRecord, ProviderOperationStore,
};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::sync::{Mutex, Notify};

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
        let mut runtimes = HashMap::with_capacity(bindings.len());
        for binding in bindings {
            let endpoint = binding.identity.endpoint.clone();
            if runtimes
                .insert(
                    endpoint,
                    RuntimeBinding {
                        identity: binding.identity,
                        runtime: Arc::new(binding.runtime),
                    },
                )
                .is_some()
            {
                return Err("external provider endpoint binding must be unique");
            }
        }
        Ok(Self {
            inner: Arc::new(SupervisorInner {
                bindings: runtimes,
                store,
                live_operations: StdMutex::new(LiveOperations::default()),
                started_at_ms: now_ms(),
                #[cfg(test)]
                admission_test_pause: StdMutex::new(None),
            }),
        })
    }

    pub async fn install_approval_broker(
        &self,
        broker: Arc<collaboration_service::ServiceApprovalBroker>,
    ) {
        for binding in self.inner.bindings.values() {
            binding
                .runtime
                .install_approval_broker(Arc::clone(&broker))
                .await;
        }
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
                binding,
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
        F: std::future::Future<Output = OperationCompletion> + Send + 'static,
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

enum OperationCompletion {
    Success {
        settlement: ConversationOperationSettlement,
        target: Option<SessionRef>,
    },
    Failure(ConversationOperationFailure),
}

impl SupervisorInner {
    async fn finish(
        &self,
        operation_id: OperationId,
        live: Arc<LiveOperation>,
        completion: OperationCompletion,
    ) {
        let result = match completion {
            OperationCompletion::Success { settlement, target } => {
                let mut store = self.store.lock().await;
                if let Some(target) = target.as_ref()
                    && store
                        .record_target(&operation_id, target, now_ms())
                        .await
                        .is_err()
                {
                    Err(applied_storage_failure(
                        operation_id.clone(),
                        Some(target.clone()),
                    ))
                } else if store
                    .record_terminal(
                        &operation_id,
                        ProviderOperationEffect::Applied,
                        ProviderReconciliationState::Confirmed,
                        now_ms(),
                    )
                    .await
                    .is_err()
                {
                    Err(applied_storage_failure(operation_id.clone(), target))
                } else {
                    Ok(settlement)
                }
            }
            OperationCompletion::Failure(operation_failure) => {
                let reconciliation = if operation_failure.effect == ProviderOperationEffect::Unknown
                {
                    ProviderReconciliationState::Unresolved
                } else {
                    ProviderReconciliationState::Confirmed
                };
                let _ = self
                    .store
                    .lock()
                    .await
                    .record_terminal(
                        &operation_id,
                        operation_failure.effect,
                        reconciliation,
                        now_ms(),
                    )
                    .await;
                Err(operation_failure)
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
                    let working_directory = PathBuf::from(String::from(request.working_directory));
                    backend.spawn_operation(operation_id.clone(), live, async move {
                        match runtime.create_session(working_directory).await {
                            Ok(provider_session_id) => {
                                match SessionId::try_from(provider_session_id) {
                                    Ok(session_id) => {
                                        let target = SessionRef {
                                            endpoint,
                                            session_id,
                                        };
                                        OperationCompletion::Success {
                                            settlement: ConversationOperationSettlement::Created {
                                                target: target.clone(),
                                                effective_settings: effective_settings(
                                                    requested_policy,
                                                ),
                                            },
                                            target: Some(target),
                                        }
                                    }
                                    Err(_) => OperationCompletion::Failure(failure(
                                        ConversationOperationFailureKind::ProtocolViolation,
                                        ConversationOperationFailureStage::Settlement,
                                        ProviderOperationEffect::Applied,
                                        "provider returned an invalid conversation identity",
                                        operation_id,
                                        None,
                                    )),
                                }
                            }
                            Err(error) => OperationCompletion::Failure(runtime_failure(
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
                    let working_directory = PathBuf::from(String::from(request.working_directory));
                    let completion_target = target.clone();
                    backend.spawn_operation(operation_id.clone(), live, async move {
                        match runtime
                            .load_session(provider_session_id, working_directory)
                            .await
                        {
                            Ok(()) => OperationCompletion::Success {
                                settlement: ConversationOperationSettlement::Loaded {
                                    target: completion_target.clone(),
                                    effective_settings: effective_settings(requested_policy),
                                },
                                target: Some(completion_target),
                            },
                            Err(error) => OperationCompletion::Failure(runtime_failure(
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
        let failure_target = Some(request.target.clone());
        let backend = self.clone();
        retain_operation(operation_id, failure_target, async move {
            let target = request.target.clone();
            let rendered_prompt = render_message(&target, &request.prompt).map_err(|_| {
                failure(
                    ConversationOperationFailureKind::InvalidRequest,
                    ConversationOperationFailureStage::Validation,
                    ProviderOperationEffect::None,
                    "provider prompt could not be rendered within the Control frame bound",
                    request.operation_id.clone(),
                    Some(target.clone()),
                )
            })?;
            let (binding, runtime) = backend.runtime_binding(
                &target.endpoint,
                &request.operation_id,
                Some(target.clone()),
            )?;
            let approval_context = crate::ExternalProviderApprovalContext {
                requester: request.requested_by.clone(),
                approver: request.approver.clone(),
                target: target.clone(),
                operation_id: request.operation_id.clone(),
                binding_generation: binding.generation.clone(),
                binding_retirement: runtime.retirement(),
            };
            let prepared = backend
                .prepare_operation(
                    request.operation_id.clone(),
                    ProviderOperationKind::ConversationPrompt,
                    binding,
                    Some(&target),
                )
                .await?;
            match prepared {
                PreparedOperation::Existing(operation) => Ok(existing_submission(operation)),
                PreparedOperation::Admitted { snapshot, live } => {
                    let operation_id = request.operation_id.clone();
                    let provider_session_id = String::from(target.session_id.clone());
                    let completion_target = target.clone();
                    backend.spawn_operation(operation_id.clone(), live, async move {
                        match runtime
                            .prompt_with_approval_context(
                                provider_session_id,
                                rendered_prompt.text,
                                approval_context,
                            )
                            .await
                        {
                            Ok(outcome) => match optional_message_text(outcome.output) {
                                Ok(response) => OperationCompletion::Success {
                                    settlement: ConversationOperationSettlement::PromptCompleted {
                                        target: completion_target.clone(),
                                        stop_reason: outcome.stop_reason,
                                        response,
                                    },
                                    target: Some(completion_target),
                                },
                                Err(_) => OperationCompletion::Failure(failure(
                                    ConversationOperationFailureKind::ProtocolViolation,
                                    ConversationOperationFailureStage::Settlement,
                                    ProviderOperationEffect::Applied,
                                    "provider returned invalid prompt output",
                                    operation_id,
                                    Some(completion_target),
                                )),
                            },
                            Err(error) => OperationCompletion::Failure(runtime_failure(
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
                || target_operation.binding.generation != request.generation
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
                            Ok(()) => OperationCompletion::Success {
                                settlement: ConversationOperationSettlement::CancelRequested {
                                    target: completion_target.clone(),
                                    target_operation_id,
                                },
                                target: Some(completion_target),
                            },
                            Err(error) => OperationCompletion::Failure(runtime_failure(
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
            snapshot_from_record(self.inspect_record(&request.operation_id).await?)
                .map_err(|message| snapshot_failure(request.operation_id, message))
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

fn effective_settings(
    requested_policy: collaboration_protocol::ProviderRequestedPolicy,
) -> EffectiveProviderSettings {
    EffectiveProviderSettings {
        requested_policy,
        mapping_status: ProviderSettingsMappingStatus::Unverified,
        authentication: ProviderAuthenticationState::Unverified,
        provider_permission_mode: None,
        permission_outcome: None,
    }
}

fn optional_message_text(output: String) -> Result<Option<MessageText>, ()> {
    if output.is_empty() {
        Ok(None)
    } else {
        MessageText::try_from(output).map(Some).map_err(|_| ())
    }
}

fn runtime_failure(
    operation_id: OperationId,
    target: Option<SessionRef>,
    error: ExternalProviderRuntimeError,
) -> ConversationOperationFailure {
    match error {
        ExternalProviderRuntimeError::LocalBusy => failure(
            ConversationOperationFailureKind::Busy,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider conversation already has active work",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::LocalNotFound
        | ExternalProviderRuntimeError::LocalCancelTargetMismatch => failure(
            ConversationOperationFailureKind::NotFound,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider conversation operation is not active",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::AuthenticationRequired => failure(
            ConversationOperationFailureKind::AuthenticationRequired,
            ConversationOperationFailureStage::Binding,
            ProviderOperationEffect::None,
            "provider authentication is required",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::ProviderFailure => failure(
            ConversationOperationFailureKind::ProviderRejected,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider rejected the operation after dispatch",
            operation_id,
            target,
        ),
        _ => failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider operation response was not available after dispatch",
            operation_id,
            target,
        ),
    }
}

fn admission_failure(
    operation_id: OperationId,
    message: &'static str,
    target: Option<SessionRef>,
) -> ConversationOperationFailure {
    failure(
        ConversationOperationFailureKind::Unavailable,
        ConversationOperationFailureStage::Admission,
        ProviderOperationEffect::None,
        message,
        operation_id,
        target,
    )
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
        operation_id,
        target,
    }
}

fn snapshot_from_record(
    record: ProviderOperationRecord,
) -> Result<ConversationOperationSnapshot, &'static str> {
    Ok(ConversationOperationSnapshot {
        operation_id: record.operation_id,
        operation: record.operation_kind,
        binding: record.binding,
        target: record.target,
        stage: record.stage,
        effect: record.effect,
        reconciliation: record.reconciliation_state,
        admitted_at: timestamp_from_millis(record.admitted_at_ms)?,
        terminal_at: record
            .terminal_at_ms
            .map(timestamp_from_millis)
            .transpose()?,
    })
}

fn timestamp_from_millis(milliseconds: i64) -> Result<ObservationTimestamp, &'static str> {
    let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(milliseconds)
        .ok_or("provider operation timestamp is outside the supported range")?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    ObservationTimestamp::try_from(timestamp)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests;
