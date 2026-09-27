//! Restore one Session with or without agent history replay.

use super::*;
use agent_client_protocol::schema::v1::ResumeSessionRequest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RestoreHistoryMode {
    Replay,
    WithoutReplay,
}

pub(super) struct RestoreAdmissionInputs<P: InteractionPort> {
    pub(super) connection: ConnectionTo<Agent>,
    pub(super) provider_session_id: String,
    pub(super) cwd: PathBuf,
    pub(super) mode: RestoreHistoryMode,
    pub(super) mcp_servers: Vec<McpServer>,
    pub(super) shutdown: CancellationToken,
    pub(super) event_sink: Arc<dyn SessionEventSink>,
    pub(super) admission_tx: tokio::sync::mpsc::Sender<PendingSessionAdmission<P>>,
    pub(super) reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
}

pub(super) async fn run_restore_admission<P: InteractionPort>(inputs: RestoreAdmissionInputs<P>) {
    let RestoreAdmissionInputs {
        connection,
        provider_session_id,
        cwd,
        mode,
        mcp_servers,
        shutdown,
        event_sink,
        admission_tx,
        reply,
    } = inputs;
    let replay_started = if mode == RestoreHistoryMode::Replay {
        tokio::select! {
            () = shutdown.cancelled() => Err(ExternalProviderRuntimeError::TransportFailure),
            result = event_sink.begin_history_replay(&provider_session_id) =>
                result.map_err(|_| ExternalProviderRuntimeError::HistoryReplayUnavailable),
        }
    } else {
        Ok(())
    };
    let result = match replay_started {
        Err(error) => Err(error),
        Ok(()) => match mode {
            RestoreHistoryMode::Replay => {
                let request = LoadSessionRequest::new(provider_session_id.clone(), &cwd)
                    .mcp_servers(mcp_servers);
                tokio::select! {
                    () = shutdown.cancelled() => Err(ExternalProviderRuntimeError::TransportFailure),
                    result = connection.load_session_from(request).block_task().start_session() =>
                        result.map(|restored| restored.into_session()).map_err(acp_load_session_error),
                }
            }
            RestoreHistoryMode::WithoutReplay => {
                let request = ResumeSessionRequest::new(provider_session_id.clone(), &cwd)
                    .mcp_servers(mcp_servers);
                tokio::select! {
                    () = shutdown.cancelled() => Err(ExternalProviderRuntimeError::TransportFailure),
                    result = connection.resume_session_from(request).block_task().start_session() =>
                        result.map(|restored| restored.into_session()).map_err(acp_operation_error),
                }
            }
        },
    };
    publish_pending_session_admission(
        &admission_tx,
        &shutdown,
        PendingSessionAdmission::Restore {
            provider_session_id,
            result: Box::new(result),
            reply,
        },
    )
    .await;
}
