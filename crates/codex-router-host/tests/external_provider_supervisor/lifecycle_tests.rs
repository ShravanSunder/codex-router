use super::*;

#[cfg(unix)]
#[tokio::test]
async fn supplied_id_is_admitted_once_and_cancelled_prompt_settles_after_detach() -> TestResult {
    let root = tempfile::tempdir()?;
    let dispatch_log = root.path().join("dispatch.log");
    let endpoint = endpoint("claude-local")?;
    let runtime = ExternalProviderRuntime::initialize(cancel_fixture(&dispatch_log)).await?;
    let backend = supervisor(&root, vec![(binding(endpoint.clone(), "claude")?, runtime)]).await?;
    let requester = actor(endpoint.clone(), "requester")?;
    let operation_id = OperationId::generate();
    let create_request = ConversationCreateRequest {
        settings: None,
        operation_id: operation_id.clone(),
        endpoint: endpoint.clone(),
        generation: Some(generation()?),
        working_directory: working_directory()?,
        created_by: (requester.clone()).into(),
        approver: (requester.clone()).into(),
        requested_policy: policy(),
    };
    let detached_create = backend.create(create_request.clone());
    drop(detached_create);
    await_admission(&backend, &operation_id).await?;
    let duplicate = operation(backend.create(create_request).await)?;
    ensure_eq!(duplicate.admission, ConversationAdmissionState::Existing);
    let created = operation(wait(&backend, operation_id).await)?;
    let target = match created.output {
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { target, .. },
        } => target,
        output => return Err(format!("unexpected create output: {output:?}").into()),
    };
    let mut stored =
        ProviderOperationStore::open(&root.path().join("provider-operations.sqlite")).await?;
    let session_record = stored
        .session_record(&target)
        .await?
        .ok_or("created session record missing")?;
    ensure_eq!(session_record.created_by, requester.clone().into());
    ensure_eq!(session_record.approver, requester.clone().into());
    stored.close().await?;

    let prompt_operation_id = OperationId::generate();
    let prompt = operation(
        backend
            .prompt(ConversationPromptRequest {
                input_id: None,
                operation_id: prompt_operation_id.clone(),
                target: target.clone(),
                generation: Some(generation()?),
                requested_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("hold".to_owned())?,
                },
            })
            .await,
    )?;
    ensure_eq!(prompt.operation.effect, ProviderOperationEffect::Unknown);
    drop(prompt);

    let cancel_operation_id = OperationId::generate();
    operation(
        backend
            .cancel(ConversationCancelRequest {
                operation_id: cancel_operation_id.clone(),
                target_operation_id: prompt_operation_id.clone(),
                target: target.clone(),
                generation: Some(generation()?),
                requested_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
            })
            .await,
    )?;
    let cancel = operation(wait(&backend, cancel_operation_id.clone()).await)?;
    ensure!(matches!(
        cancel.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::CancelRequested { .. }
        }
    ));
    let prompt = operation(wait(&backend, prompt_operation_id.clone()).await)?;
    ensure!(matches!(
        prompt.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted {
                stop_reason: ProviderPromptStopReason::Cancelled,
                ..
            }
        }
    ));
    ensure_eq!(
        std::fs::read_to_string(dispatch_log)?,
        "new\nprompt\ncancel\n"
    );
    let duplicate_cancel = operation(
        backend
            .cancel(ConversationCancelRequest {
                operation_id: cancel_operation_id,
                target_operation_id: prompt_operation_id,
                target,
                generation: Some(generation()?),
                requested_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
            })
            .await,
    )?;
    ensure_eq!(
        duplicate_cancel.admission,
        ConversationAdmissionState::Existing
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn supervisor_shutdown_joins_runtime_and_settles_held_work() -> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("claude-local")?;
    let runtime =
        ExternalProviderRuntime::initialize(cancel_fixture(&root.path().join("shutdown.log")))
            .await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "claude")?, runtime)],
    )
    .await?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let create_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: create_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_id).await)?;
    let target = match created.output {
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { target, .. },
        } => target,
        other => return Err(format!("unexpected create output: {other:?}").into()),
    };
    let prompt_id = OperationId::generate();
    operation(
        backend
            .prompt(ConversationPromptRequest {
                input_id: None,
                operation_id: prompt_id.clone(),
                target,
                generation: Some(generation()?),
                requested_by: (requester.clone()).into(),
                approver: (requester).into(),
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("hold".to_owned())?,
                },
            })
            .await,
    )?;
    await_admission(&backend, &prompt_id).await?;
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    let failure = wait(&backend, prompt_id)
        .await
        .expect_err("shutdown-held prompt cannot succeed");
    ensure_eq!(failure.effect, ProviderOperationEffect::Unknown);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn supervisor_shutdown_drains_saturated_load_completions_and_preserves_unknown_effect()
-> TestResult {
    let root = tempfile::tempdir()?;
    let admission_log = root.path().join("load-admissions.log");
    let process_id_path = root.path().join("provider.pid");
    let provider_endpoint = endpoint("claude-local")?;
    let runtime = ExternalProviderRuntime::initialize(saturated_load_fixture(
        &admission_log,
        &process_id_path,
    ))
    .await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "claude")?, runtime)],
    )
    .await?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let mut operation_ids = Vec::new();
    for index in 0..40 {
        let operation_id = OperationId::generate();
        operation(
            backend
                .load(ConversationLoadRequest {
                    operation_id: operation_id.clone(),
                    target: actor(provider_endpoint.clone(), &format!("held-load-{index}"))?,
                    generation: Some(generation()?),
                    working_directory: working_directory()?,
                    requested_by: (requester.clone()).into(),
                    approver: (requester.clone()).into(),
                    requested_policy: policy(),
                })
                .await,
        )?;
        operation_ids.push(operation_id);
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if std::fs::read_to_string(&admission_log)
                .unwrap_or_default()
                .lines()
                .count()
                == 40
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    for operation_id in operation_ids {
        let failure = wait(&backend, operation_id)
            .await
            .expect_err("shutdown-held load cannot succeed");
        ensure_eq!(failure.effect, ProviderOperationEffect::Unknown);
    }
    assert_process_reaped(&process_id_path).await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn supervisor_permission_callback_uses_installed_broker_and_exact_selected_option()
-> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let approver = actor(endpoint("codex-local")?, "approver")?;
    let runtime = ExternalProviderRuntime::initialize(permission_allow_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let (broker, native_backend) =
        approval_broker_fixture(&root, &provider_endpoint.service_id, &approver).await?;
    backend.install_approval_broker(Arc::clone(&broker)).await;

    let create_operation_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: create_operation_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: (requester.clone()).into(),
                approver: (approver.clone()).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_operation_id).await)?;
    let target = match created.output {
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { target, .. },
        } => target,
        output => return Err(format!("unexpected create output: {output:?}").into()),
    };

    let prompt_operation_id = OperationId::generate();
    operation(
        backend
            .prompt(ConversationPromptRequest {
                input_id: None,
                operation_id: prompt_operation_id.clone(),
                target,
                generation: Some(generation()?),
                requested_by: (requester).into(),
                approver: (approver.clone()).into(),
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("request permission".to_owned())?,
                },
            })
            .await,
    )?;
    let pending = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(record) = broker.list_typed_approvals(true).await.into_iter().next() {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let while_live = operation(
        backend
            .reconcile(ConversationOperationReconcileRequest {
                operation_id: prompt_operation_id.clone(),
            })
            .await,
    )?;
    ensure_eq!(while_live.stage, ProviderOperationStage::MayHaveDispatched);
    ensure_eq!(
        while_live.reconciliation,
        ProviderReconciliationState::Unresolved
    );
    ensure_eq!(
        pending
            .options
            .iter()
            .next()
            .map(|option| option.option_id.as_str()),
        Some("allow-exact-once")
    );
    let pending_history = broker.list_interactions().await;
    let Some(collaboration_service::InteractionHistoryRecord::Approval {
        requester: recorded_requester,
        approver: recorded_approver,
        ..
    }) = pending_history
        .iter()
        .find(|record| record.request_id() == pending.request_id)
    else {
        return Err("typed approval missing from history".into());
    };
    ensure_eq!(
        serde_json::to_value(recorded_requester)?,
        serde_json::to_value(actor(endpoint("cursor-local")?, "fixture-session")?)?
    );
    ensure_eq!(
        serde_json::to_value(recorded_approver)?,
        json!({"kind":"session","session":approver.clone()})
    );
    let native_requests = tokio::time::timeout(std::time::Duration::from_secs(5), native_backend)
        .await
        .map_err(|_| "native approver fixture did not complete")??
        .map_err(|error| format!("native approver fixture failed: {error}"))?;
    ensure_eq!(native_requests.len(), 4);
    let decision = broker
        .decide_typed_interaction(
            &pending.request_id,
            &serde_json::from_value(json!({"kind":"session","session":approver.clone()}))?,
            collaboration_service::TypedInteractionDecision::SelectApproval {
                option_id: "allow-exact-once".into(),
                acknowledge_persistent: false,
                note: None,
            },
        )
        .await
        .map_err(|error| format!("approval decision failed: {error}"))?;
    ensure!(matches!(decision,
        collaboration_service::TypedInteractionDecisionOutcome::ApprovalSelected { option_id }
            if option_id.as_str() == "allow-exact-once"));

    let settled = operation(wait(&backend, prompt_operation_id).await)?;
    ensure!(matches!(
        settled.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted {
                stop_reason: ProviderPromptStopReason::EndTurn,
                ..
            }
        }
    ));
    ensure_eq!(broker.list_typed_approvals(false).await.len(), 1);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn retired_provider_binding_cancels_pending_approval_before_selection() -> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let approver = actor(endpoint("codex-local")?, "approver")?;
    let runtime = ExternalProviderRuntime::initialize(permission_cancel_fixture()).await?;
    let binding_retirement = runtime.retirement();
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let (broker, native_backend) =
        approval_broker_fixture(&root, &provider_endpoint.service_id, &approver).await?;
    backend.install_approval_broker(Arc::clone(&broker)).await;
    let create_operation_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: create_operation_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: (requester.clone()).into(),
                approver: (approver.clone()).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_operation_id).await)?;
    let target = match created.output {
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { target, .. },
        } => target,
        output => return Err(format!("unexpected create output: {output:?}").into()),
    };
    let prompt_operation_id = OperationId::generate();
    operation(
        backend
            .prompt(ConversationPromptRequest {
                input_id: None,
                operation_id: prompt_operation_id.clone(),
                target,
                generation: Some(generation()?),
                requested_by: (requester).into(),
                approver: (approver.clone()).into(),
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("request permission".to_owned())?,
                },
            })
            .await,
    )?;
    let pending = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(record) = broker.list_typed_approvals(true).await.into_iter().next() {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "approval did not enter pending state")?;
    let native_requests = tokio::time::timeout(std::time::Duration::from_secs(5), native_backend)
        .await
        .map_err(|_| "native approver fixture did not complete")??
        .map_err(|error| format!("native approver fixture failed: {error}"))?;
    ensure_eq!(native_requests.len(), 4);
    binding_retirement.cancel();
    let retirement_settled = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if broker.list_typed_approvals(true).await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if retirement_settled.is_err() {
        return Err(format!(
            "retired approval remained pending: {:?}",
            broker.list_interactions().await
        )
        .into());
    }
    ensure!(matches!(
        broker
            .decide_typed_interaction(
                &pending.request_id,
                &serde_json::from_value(json!({"kind":"session","session":approver}))?,
                collaboration_service::TypedInteractionDecision::SelectApproval {
                    option_id: "allow-exact-once".into(),
                    acknowledge_persistent: false,
                    note: None,
                },
            )
            .await,
        Err(collaboration_service::InteractionHistoryError::AlreadySettled)
    ));
    let history = broker.list_interactions().await;
    ensure!(
        history
            .iter()
            .any(|record| record.request_id() == pending.request_id
                && matches!(
                    record.approval_state(),
                    Some(collaboration_service::InteractionHistoryState::Cancelled { .. })
                ))
    );
    let failure = wait(&backend, prompt_operation_id)
        .await
        .expect_err("retired runtime cannot claim a successful prompt settlement");
    ensure_eq!(
        failure.kind,
        ConversationOperationFailureKind::OutcomeUnknown
    );
    backend.shutdown().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn load_and_multiple_endpoint_bindings_are_supported() -> TestResult {
    let root = tempfile::tempdir()?;
    let claude_endpoint = endpoint("claude-local")?;
    let cursor_endpoint = endpoint("cursor-local")?;
    let claude_runtime = ExternalProviderRuntime::initialize(load_fixture()).await?;
    let cursor_runtime = ExternalProviderRuntime::initialize(load_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![
            (binding(claude_endpoint.clone(), "claude")?, claude_runtime),
            (binding(cursor_endpoint.clone(), "cursor")?, cursor_runtime),
        ],
    )
    .await?;
    ensure!(backend.binding(&claude_endpoint).is_some());
    ensure!(backend.binding(&cursor_endpoint).is_some());

    let target = actor(cursor_endpoint.clone(), "restored-session")?;
    let requester = actor(cursor_endpoint, "requester")?;
    let operation_id = OperationId::generate();
    operation(
        backend
            .load(ConversationLoadRequest {
                operation_id: operation_id.clone(),
                target: target.clone(),
                generation: Some(generation()?),
                working_directory: working_directory()?,
                requested_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let loaded = operation(wait(&backend, operation_id).await)?;
    ensure!(matches!(
        loaded.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Loaded { .. }
        }
    ));
    let mut stored =
        ProviderOperationStore::open(&root.path().join("provider-operations.sqlite")).await?;
    let session_record = stored
        .session_record(&target)
        .await?
        .ok_or("loaded session record missing")?;
    ensure_eq!(session_record.created_by, requester.clone().into());
    ensure_eq!(session_record.approver, requester.into());
    stored.close().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn resume_and_close_are_durable_operations_and_resume_requires_advertisement() -> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let target = actor(provider_endpoint.clone(), "restored-session")?;
    let unsupported = supervisor(
        &root,
        vec![(
            binding(provider_endpoint.clone(), "cursor")?,
            ExternalProviderRuntime::initialize(load_fixture()).await?,
        )],
    )
    .await?;
    let unsupported_result = unsupported
        .resume(ConversationResumeRequest {
            operation_id: OperationId::generate(),
            target: target.clone(),
            generation: Some(generation()?),
            working_directory: working_directory()?,
            requested_by: (requester.clone()).into(),
            approver: (requester.clone()).into(),
            requested_policy: policy(),
        })
        .await;
    ensure!(
        matches!(unsupported_result, Err(failure) if failure.kind == ConversationOperationFailureKind::UnsupportedCapability)
    );
    unsupported.shutdown().await?;

    let root = tempfile::tempdir()?;
    let backend = supervisor(
        &root,
        vec![(
            binding(provider_endpoint, "cursor")?,
            ExternalProviderRuntime::initialize(resume_close_fixture()).await?,
        )],
    )
    .await?;
    let resume_id = OperationId::generate();
    operation(
        backend
            .resume(ConversationResumeRequest {
                operation_id: resume_id.clone(),
                target: target.clone(),
                generation: Some(generation()?),
                working_directory: working_directory()?,
                requested_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let resumed = operation(wait(&backend, resume_id).await)?;
    ensure!(matches!(
        resumed.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Resumed {
                history: collaboration_protocol::ProviderHistoryAvailability::HistoryUnavailable,
                ..
            }
        }
    ));
    let close_id = OperationId::generate();
    operation(
        backend
            .close(ConversationCloseRequest {
                operation_id: close_id.clone(),
                target: target.clone(),
                generation: Some(generation()?),
                requested_by: (requester.clone()).into(),
                approver: (requester).into(),
            })
            .await,
    )?;
    let closed = operation(wait(&backend, close_id).await)?;
    ensure!(
        matches!(closed.output, ConversationOperationWaitOutput::Available {
        settlement: ConversationOperationSettlement::Closed { target: closed_target }
    } if closed_target == target)
    );
    backend.shutdown().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn response_loss_is_unknown_and_reconciliation_never_replays() -> TestResult {
    let root = tempfile::tempdir()?;
    let endpoint = endpoint("claude-local")?;
    let runtime = ExternalProviderRuntime::initialize(launch(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'loss-fixture','version':'1'}}})); sys.stdout.flush()
sys.stdin.readline()
"#,
    ))
    .await?;
    let backend = supervisor(&root, vec![(binding(endpoint.clone(), "claude")?, runtime)]).await?;
    let requester = actor(endpoint.clone(), "requester")?;
    let operation_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: operation_id.clone(),
                endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: (requester.clone()).into(),
                approver: (requester).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let failure = match wait(&backend, operation_id.clone()).await {
        Ok(result) => return Err(format!("lost response unexpectedly settled: {result:?}").into()),
        Err(failure) => failure,
    };
    ensure_eq!(failure.effect, ProviderOperationEffect::Unknown);
    let reconciled = operation(
        backend
            .reconcile(ConversationOperationReconcileRequest { operation_id })
            .await,
    )?;
    ensure_eq!(
        reconciled.reconciliation,
        ProviderReconciliationState::NotReconcilable
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn authentication_required_is_no_effect_and_does_not_poison_fresh_create() -> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let classification_runtime =
        ExternalProviderRuntime::initialize(authentication_then_create_fixture()).await?;
    let classification = classification_runtime
        .create_session(PathBuf::from("/tmp"))
        .await;
    if !matches!(
        classification,
        Err(
            codex_router_host::ExternalProviderRuntimeError::AuthenticationRequired {
                code: -32000,
                ..
            }
        )
    ) {
        return Err(format!("unexpected authentication classification: {classification:?}").into());
    }
    classification_runtime.shutdown().await;
    let runtime = ExternalProviderRuntime::initialize(authentication_then_create_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let first_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: first_id.clone(),
                endpoint: provider_endpoint.clone(),
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let failure = wait(&backend, first_id.clone())
        .await
        .expect_err("authentication required");
    ensure_eq!(
        failure.kind,
        ConversationOperationFailureKind::AuthenticationRequired
    );
    ensure_eq!(failure.effect, ProviderOperationEffect::None);
    let duplicate = operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: first_id,
                endpoint: provider_endpoint.clone(),
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    ensure_eq!(duplicate.admission, ConversationAdmissionState::Existing);
    let second_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: second_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: (requester.clone()).into(),
                approver: (requester).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, second_id).await)?;
    ensure!(matches!(
        created.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { .. }
        }
    ));
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn prompt_authentication_required_is_no_effect_and_fresh_prompt_retains_final_text()
-> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let runtime = ExternalProviderRuntime::initialize(authentication_then_prompt_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let create_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: create_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_id).await)?;
    let ConversationOperationWaitOutput::Available {
        settlement: ConversationOperationSettlement::Created { target, .. },
    } = created.output
    else {
        return Err("provider session was not created".into());
    };

    let first_id = OperationId::generate();
    let prompt_request = |operation_id: OperationId| ConversationPromptRequest {
        input_id: None,
        operation_id,
        target: target.clone(),
        generation: Some(generation().expect("generation")),
        requested_by: (requester.clone()).into(),
        approver: (requester.clone()).into(),
        prompt: MessageContent::HumanUser {
            text: MessageText::try_from("continue".to_owned()).expect("prompt"),
        },
    };
    operation(backend.prompt(prompt_request(first_id.clone())).await)?;
    let failure = wait(&backend, first_id.clone())
        .await
        .expect_err("prompt authentication required");
    ensure_eq!(
        failure.kind,
        ConversationOperationFailureKind::AuthenticationRequired
    );
    ensure_eq!(failure.effect, ProviderOperationEffect::None);
    let duplicate = operation(backend.prompt(prompt_request(first_id)).await)?;
    ensure_eq!(duplicate.admission, ConversationAdmissionState::Existing);

    let second_id = OperationId::generate();
    operation(backend.prompt(prompt_request(second_id.clone())).await)?;
    let completed = operation(wait(&backend, second_id).await)?;
    ensure!(matches!(
        completed.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted {
                response: Some(response),
                stop_reason: ProviderPromptStopReason::EndTurn,
                ..
            }
        } if String::from(response.clone()) == "final text retained"
    ));
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn provider_prompt_error_text_cannot_become_a_local_no_effect_rejection() -> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let runtime = ExternalProviderRuntime::initialize(provider_prompt_rejection_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let create_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: create_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: (requester.clone()).into(),
                approver: (requester.clone()).into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_id).await)?;
    let ConversationOperationWaitOutput::Available {
        settlement: ConversationOperationSettlement::Created { target, .. },
    } = created.output
    else {
        return Err("provider session was not created".into());
    };
    let prompt_id = OperationId::generate();
    operation(
        backend
            .prompt(ConversationPromptRequest {
                input_id: None,
                operation_id: prompt_id.clone(),
                target,
                generation: Some(generation()?),
                requested_by: (requester.clone()).into(),
                approver: (requester).into(),
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("continue".to_owned())?,
                },
            })
            .await,
    )?;
    let failure = wait(&backend, prompt_id)
        .await
        .expect_err("provider rejection");
    ensure_eq!(
        failure.kind,
        ConversationOperationFailureKind::ProviderRejected
    );
    ensure_eq!(failure.effect, ProviderOperationEffect::Unknown);
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    Ok(())
}
