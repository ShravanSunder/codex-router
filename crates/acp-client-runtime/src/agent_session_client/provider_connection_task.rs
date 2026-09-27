//! Own the ACP connection task, provider process, and Session admission loop.

use super::*;

impl<P: InteractionPort> AgentSessionClient<P> {
    pub async fn initialize(
        launch: ExternalProviderLaunch,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            INITIALIZE_TIMEOUT,
            Vec::new(),
            interaction_port,
            event_sink,
        )
        .await
    }

    pub async fn initialize_with_mcp_http(
        launch: ExternalProviderLaunch,
        server_name: impl Into<String>,
        server_url: impl Into<String>,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            INITIALIZE_TIMEOUT,
            vec![McpServer::Http(McpServerHttp::new(server_name, server_url))],
            interaction_port,
            event_sink,
        )
        .await
    }

    #[cfg(any(test, feature = "test-observation"))]
    pub async fn initialize_with_timeout(
        launch: ExternalProviderLaunch,
        initialize_timeout: Duration,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            initialize_timeout,
            Vec::new(),
            interaction_port,
            event_sink,
        )
        .await
    }

    async fn initialize_with_timeout_and_mcp_servers(
        launch: ExternalProviderLaunch,
        initialize_timeout: Duration,
        configured_mcp_servers: Vec<McpServer>,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        let agent = AcpAgent::new(launch.sdk_config());
        let persistence_target = launch.persistence_target;
        let (stdin, stdout, mut stderr, mut child) = agent
            .spawn_process()
            .map_err(|error| ExternalProviderRuntimeError::Launch(error.to_string()))?;
        let shutdown = CancellationToken::new();
        let retirement = CancellationToken::new();
        let frame_observation = Arc::new(ProviderFrameObservation::default());
        let task_frame_observation = Arc::clone(&frame_observation);
        let shutdown_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_shutdown_failed = Arc::clone(&shutdown_failed);
        let task_shutdown = shutdown.clone();
        let connection_retirement = retirement.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(32);
        let session_capabilities = Arc::new(tokio::sync::RwLock::new(HashMap::<
            String,
            ProviderCapabilityReport,
        >::new()));
        let task_session_capabilities = Arc::clone(&session_capabilities);
        let session_settings = Arc::new(tokio::sync::RwLock::new(HashMap::<
            String,
            crate::ProviderSettingsCatalog,
        >::new()));
        let task_session_settings = Arc::clone(&session_settings);
        let last_settings_catalog = Arc::new(tokio::sync::RwLock::new(None));
        let task_last_settings_catalog = Arc::clone(&last_settings_catalog);
        let task_event_sink = Arc::clone(&event_sink);
        #[cfg(any(test, feature = "test-observation"))]
        let permission_request_count = Arc::new(AtomicU64::new(0));
        #[cfg(any(test, feature = "test-observation"))]
        let callback_permission_request_count = Arc::clone(&permission_request_count);
        #[cfg(any(test, feature = "test-observation"))]
        let permission_outcome = Arc::new(std::sync::atomic::AtomicU8::new(0));
        #[cfg(any(test, feature = "test-observation"))]
        let callback_permission_outcome = Arc::clone(&permission_outcome);
        let callback_interaction_port = Arc::clone(&interaction_port);
        let task_interaction_port = Arc::clone(&interaction_port);
        let approval_contexts = Arc::new(std::sync::Mutex::new(HashMap::<
            String,
            ActiveApprovalContext<P>,
        >::new()));
        let callback_approval_contexts = Arc::clone(&approval_contexts);
        let task_approval_contexts = Arc::clone(&approval_contexts);
        let known_sessions = ProviderKnownSessions::default();
        let request_known_sessions = known_sessions.clone();
        let permission_refusal_reasons = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let callback_permission_refusal_reasons = Arc::clone(&permission_refusal_reasons);
        let endpoint_id = Arc::new(tokio::sync::RwLock::new(None::<String>));
        let callback_endpoint_id = Arc::clone(&endpoint_id);
        #[cfg(any(test, feature = "test-observation"))]
        let approval_refusal_warnings = Arc::new(std::sync::Mutex::new(Vec::<
            ExternalProviderApprovalRefusalWarning,
        >::new()));
        #[cfg(any(test, feature = "test-observation"))]
        let callback_approval_refusal_warnings = Arc::clone(&approval_refusal_warnings);
        #[cfg(any(test, feature = "test-observation"))]
        let test_tool_calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        #[cfg(any(test, feature = "test-observation"))]
        let session_test_tool_calls = Arc::clone(&test_tool_calls);
        let task = tokio::spawn(async move {
            use futures_util::StreamExt as _;
            use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
            use tokio_util::compat::{
                FuturesAsyncReadCompatExt as _, FuturesAsyncWriteCompatExt as _,
            };

            let stderr_task = tokio::spawn(async move {
                use futures_util::io::AsyncReadExt as _;
                let mut buffer = [0_u8; 8 * 1024];
                while stderr.read(&mut buffer).await.unwrap_or(0) != 0 {}
            });
            let outgoing = futures_util::SinkExt::<String>::sink_map_err(
                FramedWrite::new(stdin.compat_write(), LinesCodec::new()),
                std::io::Error::other,
            );
            let incoming_frame_observation = Arc::clone(&task_frame_observation);
            let incoming = FramedRead::new(
                stdout.compat(),
                LinesCodec::new_with_max_length(MAX_ACP_FRAME_BYTES),
            )
            .map(move |line| {
                line.map_err(|error| {
                    if matches!(
                        error,
                        tokio_util::codec::LinesCodecError::MaxLineLengthExceeded
                    ) {
                        incoming_frame_observation.record_limit_exceeded();
                    }
                    std::io::Error::other(error)
                })
            })
            .chain(futures_util::stream::once(async {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "provider stdout closed",
                ))
            }));
            let final_interaction_port = Arc::clone(&callback_interaction_port);
            let connection = Client.builder().name("codex-router-host")
                .with_handler(ProviderRequestSessionGuard::new(request_known_sessions))
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder, connection| {
                        #[cfg(any(test, feature = "test-observation"))]
                        callback_permission_request_count.fetch_add(1, Ordering::Relaxed);
                        let context = callback_approval_contexts.lock().ok().and_then(|contexts| {
                            contexts.get(request.session_id.0.as_ref()).cloned()
                        });
                        let dispatch_state = PermissionDispatchState {
                            interaction_port: Arc::clone(&callback_interaction_port),
                            refusal_reasons: Arc::clone(&callback_permission_refusal_reasons),
                            endpoint_id: Arc::clone(&callback_endpoint_id),
                            persistence_target,
                            #[cfg(any(test, feature = "test-observation"))]
                            refusal_warnings: Arc::clone(&callback_approval_refusal_warnings),
                            #[cfg(any(test, feature = "test-observation"))]
                            permission_outcome: Arc::clone(&callback_permission_outcome),
                        };
                        if context.is_none() {
                            record_permission_refusal(
                                &dispatch_state,
                                request.session_id.0.as_ref(),
                                None,
                                ExternalProviderApprovalRefusalReason::MissingPromptContext,
                            )
                            .await;
                        }
                        spawn_external_approval_dispatch(
                            request,
                            responder,
                            connection,
                            context,
                            dispatch_state,
                        )
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .with_handler(ProviderRequestFallback)
                .connect_with(
                Lines::new(outgoing, incoming),
                async move |connection| {
                    let initialized = tokio::select! {
                        biased;
                        () = task_shutdown.cancelled() => {
                            return Ok(());
                        }
                        response = initialize_provider_connection(&connection) => response,
                    };
                    let admission = match initialized {
                        Ok(response) if response.protocol_version == ProtocolVersion::V1 => {
                            let capability_report = ProviderCapabilityReport::from_initialize(&response);
                            Ok((ExternalProviderAdmission {
                                runtime_name: response
                                    .agent_info
                                    .as_ref()
                                    .map(|info| info.name.clone()),
                                runtime_version: response
                                    .agent_info
                                    .as_ref()
                                    .map(|info| info.version.clone()),
                                supports_load: response.agent_capabilities.load_session,
                                supports_mcp_http: response
                                    .agent_capabilities
                                    .mcp_capabilities
                                    .http,
                                supports_steering: response.meta.as_ref()
                                    .and_then(|meta| meta.get("steering"))
                                    .and_then(|steering| steering.get("supported"))
                                    .and_then(serde_json::Value::as_bool)
                                    == Some(true),
                            }, capability_report))
                        }
                        Ok(response) => Err(ExternalProviderRuntimeError::UnsupportedProtocol {
                            actual: AcpProtocolVersion::from_sdk(response.protocol_version),
                        }),
                        Err(error) => {
                            Err(ExternalProviderRuntimeError::Initialize(
                                sanitized_initialization_error(&error),
                            ))
                        }
                    };
                    let admitted = admission.is_ok();
                    let session_mcp_servers = if matches!(
                        &admission,
                        Ok((admission, _)) if admission.supports_mcp_http
                    ) {
                        configured_mcp_servers
                    } else {
                        Vec::new()
                    };
                    let base_capabilities = admission.as_ref().ok().map(|(_, report)| report.clone());
                    let _result = ready_tx.send(admission);
                    if admitted {
                        let mut sessions = HashMap::<
                            String,
                            tokio::sync::mpsc::Sender<ProviderSessionCommand<P>>,
                        >::new();
                        let mut session_tasks = tokio::task::JoinSet::new();
                        let mut admission_tasks = tokio::task::JoinSet::new();
                        let (admission_tx, mut admission_rx) = tokio::sync::mpsc::channel(32);
                        let mut pending_loads = std::collections::HashSet::<String>::new();
                        let Some(base_capabilities) = base_capabilities else { return Ok(()); };
                        loop {
                            tokio::select! {
                                () = task_shutdown.cancelled() => break,
                                completion = admission_rx.recv() => {
                                    let Some(completion) = completion else { continue; };
                                    match completion {
                                        PendingSessionAdmission::Create { result, reply } => {
                                            let report = result.as_ref().as_ref().ok().map(|registration| {
                                                base_capabilities.with_session_response(&registration.response)
                                            });
                                            let catalog = result.as_ref().as_ref().ok().map(|registration| {
                                                crate::provider_settings_catalog_codec::catalog_from_session_response(&registration.response)
                                            });
                                            let mut result = (*result).and_then(|registration| {
                                                register_provider_session(registration, &mut sessions)
                                            });
                                            if let Ok(created) = &mut result {
                                                known_sessions.track(created.provider_session_id.clone()).await;
                                                if let Some(report) = report {
                                                    task_session_capabilities.write().await.insert(created.provider_session_id.clone(), report);
                                                }
                                                if let Some(catalog) = catalog {
                                                    created.effective_settings = catalog.effective_settings();
                                                    task_session_settings.write().await.insert(created.provider_session_id.clone(), catalog.clone());
                                                    *task_last_settings_catalog.write().await = Some(catalog);
                                                }
                                            }
                                            let _result = reply.send(result);
                                        }
                                        PendingSessionAdmission::Load { provider_session_id, result, reply } => {
                                            pending_loads.remove(&provider_session_id);
                                            let report = result.as_ref().as_ref().ok().map(|session| {
                                                base_capabilities.with_session_response(&session.response())
                                            });
                                            let catalog = result.as_ref().as_ref().ok().map(|session| {
                                                crate::provider_settings_catalog_codec::catalog_from_session_response(&session.response())
                                            });
                                            let result = (*result).and_then(|mut session| {
                                                discard_queued_session_updates(&mut session)?;
                                                register_static_provider_session(
                                                    session,
                                                    &mut sessions,
                                                    &mut session_tasks,
                                                    task_shutdown.clone(),
                                                    Arc::clone(&task_frame_observation),
                                                    #[cfg(any(test, feature = "test-observation"))] Arc::clone(&session_test_tool_calls),
                                                )
                                            }).map(|_| ());
                                            if result.is_err() {
                                                known_sessions.forget(&provider_session_id).await;
                                            } else if let Some(report) = report {
                                                task_session_capabilities.write().await.insert(provider_session_id.clone(), report);
                                                if let Some(catalog) = catalog {
                                                    task_session_settings.write().await.insert(provider_session_id.clone(), catalog.clone());
                                                    *task_last_settings_catalog.write().await = Some(catalog);
                                                }
                                            }
                                            let _result = reply.send(result);
                                        }
                                    }
                                }
                                completion = admission_tasks.join_next(), if !admission_tasks.is_empty() => {
                                    if matches!(completion, Some(Err(_))) {
                                        task_shutdown_failed.store(true, Ordering::Relaxed);
                                        break;
                                    }
                                }
                                command = command_rx.recv() => {
                                    let Some(command) = command else { break; };
                                    match command {
                                        ProviderCommand::Create { cwd, reply } => {
                                            let request = NewSessionRequest::new(&cwd)
                                                .mcp_servers(session_mcp_servers.clone());
                                            let pending_connection = connection.clone();
                                            let pending_shutdown = task_shutdown.clone();
                                            let pending_admission_tx = admission_tx.clone();
                                            let pending_frame_observation = Arc::clone(&task_frame_observation);
                                            #[cfg(any(test, feature = "test-observation"))]
                                            let pending_test_tool_calls = Arc::clone(&session_test_tool_calls);
                                            admission_tasks.spawn(async move {
                                                run_create_admission(
                                                    pending_connection,
                                                    request,
                                                    pending_shutdown,
                                                    pending_admission_tx,
                                                    reply,
                                                    pending_frame_observation,
                                                    #[cfg(any(test, feature = "test-observation"))] pending_test_tool_calls,
                                                )
                                                .await;
                                            });
                                        }
                                        ProviderCommand::Load { provider_session_id, cwd, reply } => {
                                            if sessions.contains_key(&provider_session_id) || !pending_loads.insert(provider_session_id.clone()) {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalBusy));
                                            } else {
                                                known_sessions.track(provider_session_id.clone()).await;
                                                let request = LoadSessionRequest::new(
                                                    provider_session_id.clone(),
                                                    &cwd,
                                                )
                                                .mcp_servers(session_mcp_servers.clone());
                                                let pending_connection = connection.clone();
                                                let pending_shutdown = task_shutdown.clone();
                                                let pending_admission_tx = admission_tx.clone();
                                                let pending_event_sink = Arc::clone(&task_event_sink);
                                                admission_tasks.spawn(async move {
                                                    let replay_started = tokio::select! {
                                                        () = pending_shutdown.cancelled() => Err(ExternalProviderRuntimeError::TransportFailure),
                                                        result = pending_event_sink.begin_history_replay(&provider_session_id) =>
                                                            result.map_err(|_| ExternalProviderRuntimeError::HistoryReplayUnavailable),
                                                    };
                                                    let result = match replay_started {
                                                        Ok(()) => tokio::select! {
                                                            () = pending_shutdown.cancelled() => Err(ExternalProviderRuntimeError::TransportFailure),
                                                            result = pending_connection
                                                            .load_session_from(request)
                                                            .block_task()
                                                            .start_session() => result.map(|restored| restored.into_session()).map_err(acp_load_session_error),
                                                        },
                                                        Err(error) => Err(error),
                                                    };
                                                    publish_pending_session_admission(
                                                        &pending_admission_tx,
                                                        &pending_shutdown,
                                                        PendingSessionAdmission::Load {
                                                            provider_session_id,
                                                            result: Box::new(result),
                                                            reply,
                                                        },
                                                    ).await;
                                                });
                                            }
                                        }
                                        ProviderCommand::Prompt { provider_session_id, operation_id, prompt, dispatch, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                if let Some(dispatch) = dispatch {
                                                    let _result = dispatch.send(ProviderPromptDispatchObservation::NotSubmitted);
                                                }
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                                                continue;
                                            };
                                            let turn_cancellation = operation_id.as_ref().and_then(|operation_id| active_turn_cancellation(&task_approval_contexts, &task_interaction_port, &provider_session_id, Some(operation_id)));
                                            if let Err(error) = session.send(ProviderSessionCommand::Prompt { operation_id, prompt, turn_cancellation, dispatch, reply }).await
                                                && let ProviderSessionCommand::Prompt { dispatch: Some(dispatch), .. } = error.0
                                            {
                                                let _result = dispatch.send(ProviderPromptDispatchObservation::NotSubmitted);
                                            }
                                        }
                                        ProviderCommand::Cancel { provider_session_id, expected_operation_id, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                                                continue;
                                            };
                                            let _result = session.send(ProviderSessionCommand::Cancel { expected_operation_id, reply }).await;
                                        }
                                        ProviderCommand::Steer { provider_session_id, prompt, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                                                continue;
                                            };
                                            let _result = session.send(ProviderSessionCommand::Steer { prompt, reply }).await;
                                        }
                                        ProviderCommand::InspectSession { provider_session_id, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                let _result = reply.send(ProviderSessionActivity::NotLoaded);
                                                continue;
                                            };
                                            let _result = session.send(ProviderSessionCommand::Inspect { reply }).await;
                                        }
                                        ProviderCommand::WaitSessionIdle { provider_session_id, reply } => {
                                            let Some(session) = sessions.get(&provider_session_id) else {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                                                continue;
                                            };
                                            let _result = session.send(ProviderSessionCommand::WaitIdle { reply }).await;
                                        }
                                    }
                                }
                            }
                        }
                        admission_rx.close();
                        while let Ok(completion) = admission_rx.try_recv() {
                            fail_pending_session_admission(completion);
                        }
                        task_shutdown.cancel();
                        while let Some(result) = admission_tasks.join_next().await {
                            if result.is_err() {
                                task_shutdown_failed.store(true, Ordering::Relaxed);
                            }
                        }
                        while let Some(result) = session_tasks.join_next().await {
                            if result.is_err() {
                                task_shutdown_failed.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                    Ok(())
                },
            );
            let child_exited = tokio::select! {
                _result = connection => false,
                _status = child.status() => true,
            };
            connection_retirement.cancel();
            final_interaction_port.cancel_retired().await;
            #[cfg(unix)]
            if !child_exited
                && let Some(process_id) = rustix::process::Pid::from_raw(child.id().cast_signed())
            {
                let _result =
                    rustix::process::kill_process_group(process_id, rustix::process::Signal::KILL);
            }
            if !child_exited {
                let _result = child.kill();
                let _result = child.status().await;
            }
            let _result = stderr_task.await;
        });

        let (admission, base_capabilities) =
            match tokio::time::timeout(initialize_timeout, ready_rx).await {
                Ok(Ok(Ok(admission))) => admission,
                Ok(Ok(Err(error))) => {
                    shutdown.cancel();
                    let _result = task.await;
                    return Err(error);
                }
                Ok(Err(_)) => {
                    shutdown.cancel();
                    let _result = task.await;
                    return Err(ExternalProviderRuntimeError::Initialize(
                        "provider connection closed before initialization".to_owned(),
                    ));
                }
                Err(_) => {
                    shutdown.cancel();
                    let _result = task.await;
                    return Err(ExternalProviderRuntimeError::InitializeTimeout);
                }
            };
        Ok(Self {
            admission,
            base_capabilities,
            session_capabilities,
            session_settings,
            last_settings_catalog,
            shutdown,
            retirement,
            task: tokio::sync::Mutex::new(Some(task)),
            shutdown_failed,
            frame_observation,
            commands: command_tx,
            #[cfg(any(test, feature = "test-observation"))]
            permission_request_count,
            #[cfg(any(test, feature = "test-observation"))]
            permission_outcome,
            interaction_port,
            _event_sink: event_sink,
            approval_contexts,
            permission_refusal_reasons,
            endpoint_id,
            #[cfg(any(test, feature = "test-observation"))]
            approval_refusal_warnings,
            #[cfg(any(test, feature = "test-observation"))]
            test_tool_calls,
        })
    }
}
