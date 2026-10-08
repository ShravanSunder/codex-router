use super::*;

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
        match ProviderPromptContentsRequest::from_prompt_request(request) {
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
