//! Register and settle concurrent provider Session admissions.

use super::*;

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
    })
}

pub(super) fn register_static_provider_session<P: InteractionPort>(
    session: ActiveSession<'static, Agent>,
    sessions: &mut HashMap<String, tokio::sync::mpsc::Sender<ProviderSessionCommand<P>>>,
    session_tasks: &mut tokio::task::JoinSet<()>,
    shutdown: CancellationToken,
    frame_observation: Arc<ProviderFrameObservation>,
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
        #[cfg(any(test, feature = "test-observation"))]
        test_tool_calls,
    ));
    Ok(ExternalProviderCreatedSession {
        provider_session_id,
    })
}

pub(super) async fn run_create_admission<P: InteractionPort>(
    connection: ConnectionTo<Agent>,
    request: NewSessionRequest,
    shutdown: CancellationToken,
    admission_tx: tokio::sync::mpsc::Sender<PendingSessionAdmission<P>>,
    reply: tokio::sync::oneshot::Sender<
        Result<ExternalProviderCreatedSession, ExternalProviderRuntimeError>,
    >,
    frame_observation: Arc<ProviderFrameObservation>,
    #[cfg(any(test, feature = "test-observation"))] test_tool_calls: Arc<
        std::sync::Mutex<Vec<ExternalProviderToolCall>>,
    >,
) {
    let (registration_tx, registration_rx) = tokio::sync::oneshot::channel();
    let session_shutdown = shutdown.clone();
    let session_frame_observation = Arc::clone(&frame_observation);
    let session_future = connection
        .build_session_from(request)
        .block_task()
        .run_until(async move |session| {
            let provider_session_id = session.session_id().to_string();
            let (commands, command_rx) = tokio::sync::mpsc::channel(16);
            registration_tx
                .send(ProviderSessionRegistration {
                    provider_session_id,
                    commands,
                    response: session.response(),
                })
                .map_err(|_| agent_client_protocol::Error::internal_error())?;
            run_provider_session(
                session,
                command_rx,
                session_shutdown,
                session_frame_observation,
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
        registration = registration_rx => registration
            .map_err(|_| ExternalProviderRuntimeError::TransportFailure),
        result = &mut session_future => Err(result
            .err()
            .map_or(ExternalProviderRuntimeError::TransportFailure, acp_operation_error)),
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
        PendingSessionAdmission::Load { reply, .. } => {
            let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
        }
    }
}

pub(super) fn discard_queued_session_updates(
    session: &mut ActiveSession<'static, Agent>,
) -> Result<(), ExternalProviderRuntimeError> {
    use futures_util::FutureExt as _;

    loop {
        match session.read_update().now_or_never() {
            Some(Ok(_update)) => continue,
            Some(Err(error)) => {
                return Err(provider_frame_decode_error(error));
            }
            None => return Ok(()),
        }
    }
}
