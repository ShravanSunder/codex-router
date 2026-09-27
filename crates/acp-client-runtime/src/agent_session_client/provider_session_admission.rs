//! Register and settle concurrent provider Session admissions.

use super::provider_setting_application::{SettingSetupFailure, apply_initial_settings};
use super::*;
use agent_client_protocol::schema::v1::CloseSessionRequest;

pub(super) fn register_provider_session<P: InteractionPort>(
    registration: ProviderSessionRegistration<P>,
    sessions: &mut HashMap<String, tokio::sync::mpsc::Sender<ProviderSessionCommand<P>>>,
) -> Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError> {
    if sessions.contains_key(&registration.provider_session_id) {
        return Err(ExternalProviderRuntimeError::Operation(
            "provider returned a duplicate conversation identity".to_owned(),
        ));
    }
    sessions.insert(
        registration.provider_session_id.clone(),
        registration.commands,
    );
    Ok(ExternalProviderCreatedSession {
        provider_session_id: registration.provider_session_id,
        effective_settings: registration.settings_catalog.effective_settings(),
    })
}

pub(super) fn register_static_provider_session<P: InteractionPort>(
    session: ActiveSession<'static, Agent>,
    sessions: &mut HashMap<String, tokio::sync::mpsc::Sender<ProviderSessionCommand<P>>>,
    session_tasks: &mut tokio::task::JoinSet<()>,
    shutdown: CancellationToken,
    frame_observation: Arc<ProviderFrameObservation>,
    runtime_handles: ProviderSessionRuntimeHandles,
    #[cfg(any(test, feature = "test-observation"))] test_tool_calls: Arc<
        std::sync::Mutex<Vec<ExternalProviderToolCall>>,
    >,
) -> Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError> {
    let provider_session_id = session.session_id().to_string();
    if sessions.contains_key(&provider_session_id) {
        return Err(ExternalProviderRuntimeError::Operation(
            "provider returned a duplicate conversation identity".to_owned(),
        ));
    }
    let (commands, command_rx) = tokio::sync::mpsc::channel(16);
    sessions.insert(provider_session_id.clone(), commands);
    session_tasks.spawn(run_provider_session(
        session,
        command_rx,
        shutdown,
        frame_observation,
        runtime_handles,
        #[cfg(any(test, feature = "test-observation"))]
        test_tool_calls,
    ));
    Ok(ExternalProviderCreatedSession {
        provider_session_id,
        effective_settings: crate::EffectiveProviderSettings::default(),
    })
}

pub(super) struct CreateAdmissionInputs<P: InteractionPort> {
    pub(super) connection: ConnectionTo<Agent>,
    pub(super) request: NewSessionRequest,
    pub(super) requested_settings: crate::RequestedProviderSettings,
    pub(super) supports_close: bool,
    pub(super) shutdown: CancellationToken,
    pub(super) admission_tx: tokio::sync::mpsc::Sender<PendingSessionAdmission<P>>,
    pub(super) reply: tokio::sync::oneshot::Sender<
        Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError>,
    >,
    pub(super) frame_observation: Arc<ProviderFrameObservation>,
    pub(super) runtime_handles: ProviderSessionRuntimeHandles,
    #[cfg(any(test, feature = "test-observation"))]
    pub(super) test_tool_calls: Arc<std::sync::Mutex<Vec<ExternalProviderToolCall>>>,
}

pub(super) async fn run_create_admission<P: InteractionPort>(inputs: CreateAdmissionInputs<P>) {
    let CreateAdmissionInputs {
        connection,
        request,
        requested_settings,
        supports_close,
        shutdown,
        admission_tx,
        reply,
        frame_observation,
        runtime_handles,
        #[cfg(any(test, feature = "test-observation"))]
        test_tool_calls,
    } = inputs;
    let (registration_tx, mut registration_rx) = tokio::sync::oneshot::channel::<
        Result<ProviderSessionRegistration<P>, ExternalProviderRuntimeError>,
    >();
    let session_shutdown = shutdown.clone();
    let session_frame_observation = Arc::clone(&frame_observation);
    let session_future = connection
        .build_session_from(request)
        .block_task()
        .run_until(async move |session| {
            let provider_session_id = session.session_id().to_string();
            let response = session.response();
            let mut settings_catalog =
                crate::provider_settings_catalog_codec::catalog_from_session_response(&response);
            let (applied, failure) =
                apply_initial_settings(&session, &requested_settings, &mut settings_catalog).await;
            let setup_error = match failure {
                None => None,
                Some(SettingSetupFailure::Invalid {
                    kind,
                    value,
                    advertised,
                }) => {
                    let disposition = if applied.is_empty() && supports_close {
                        let close = session
                            .connection()
                            .send_request_to(
                                Agent,
                                CloseSessionRequest::new(session.session_id().clone()),
                            )
                            .block_task()
                            .await;
                        if close.is_ok() {
                            crate::InvalidSettingSessionDisposition::Closed
                        } else {
                            crate::InvalidSettingSessionDisposition::RemainsCreated
                        }
                    } else {
                        crate::InvalidSettingSessionDisposition::RemainsCreated
                    };
                    let error = if applied.is_empty() {
                        ExternalProviderRuntimeError::InvalidSetting {
                            setting: kind,
                            value,
                            advertised,
                            provider_session_id: provider_session_id.clone(),
                            disposition,
                        }
                    } else {
                        ExternalProviderRuntimeError::CreatedWithoutSettings {
                            provider_session_id: provider_session_id.clone(),
                            applied,
                            failed: crate::FailedProviderSetting {
                                kind,
                                value,
                                reason: "invalidSetting".to_owned(),
                            },
                        }
                    };
                    if disposition == crate::InvalidSettingSessionDisposition::Closed {
                        registration_tx
                            .send(Err(error))
                            .map_err(|_| agent_client_protocol::Error::internal_error())?;
                        return Ok(());
                    }
                    Some(error)
                }
                Some(SettingSetupFailure::Agent {
                    kind,
                    value,
                    reason,
                }) => Some(ExternalProviderRuntimeError::CreatedWithoutSettings {
                    provider_session_id: provider_session_id.clone(),
                    applied,
                    failed: crate::FailedProviderSetting {
                        kind,
                        value,
                        reason,
                    },
                }),
                Some(SettingSetupFailure::Uncertain { kind, value }) => {
                    Some(ExternalProviderRuntimeError::CreatedWithoutSettings {
                        provider_session_id: provider_session_id.clone(),
                        applied,
                        failed: crate::FailedProviderSetting {
                            kind,
                            value,
                            reason: "outcomeUnknown".to_owned(),
                        },
                    })
                }
            };
            let (commands, command_rx) = tokio::sync::mpsc::channel(16);
            registration_tx
                .send(Ok(ProviderSessionRegistration {
                    provider_session_id,
                    commands,
                    response,
                    settings_catalog,
                    setup_error,
                }))
                .map_err(|_| agent_client_protocol::Error::internal_error())?;
            run_provider_session(
                session,
                command_rx,
                session_shutdown,
                session_frame_observation,
                runtime_handles,
                #[cfg(any(test, feature = "test-observation"))]
                test_tool_calls,
            )
            .await;
            Ok(())
        });
    tokio::pin!(session_future);
    let result = tokio::select! {
        biased;
        () = shutdown.cancelled() => Err(ExternalProviderRuntimeError::TransportFailure),
        registration = &mut registration_rx => registration
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure)
            .and_then(std::convert::identity),
        result = &mut session_future => match registration_rx.try_recv() {
            Ok(registration) => registration,
            Err(_) => Err(result
                .err()
                .map_or(ExternalProviderRuntimeError::TransportFailure, acp_operation_error)),
        },
    };
    let registered = result.is_ok();
    publish_pending_session_admission(
        &admission_tx,
        &shutdown,
        PendingSessionAdmission::Create {
            result: Box::new(result),
            reply,
        },
    )
    .await;
    if registered {
        let _result = tokio::select! {
            () = shutdown.cancelled() => Ok(()),
            result = &mut session_future => result,
        };
    }
}

pub(super) async fn publish_pending_session_admission<P: InteractionPort>(
    admission_tx: &tokio::sync::mpsc::Sender<PendingSessionAdmission<P>>,
    shutdown: &CancellationToken,
    completion: PendingSessionAdmission<P>,
) {
    tokio::select! {
        biased;
        () = shutdown.cancelled() => fail_pending_session_admission(completion),
        permit = admission_tx.reserve() => match permit {
            Ok(permit) => permit.send(completion),
            Err(_) => fail_pending_session_admission(completion),
        },
    }
}

pub(super) fn fail_pending_session_admission<P: InteractionPort>(
    completion: PendingSessionAdmission<P>,
) {
    match completion {
        PendingSessionAdmission::Create { reply, .. } => {
            let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
        }
        PendingSessionAdmission::Restore { reply, .. }
        | PendingSessionAdmission::Close { reply, .. } => {
            let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
        }
    }
}
