//! Restore one Session with or without agent history replay.

use super::*;
use agent_client_protocol::schema::v1::{ResumeSessionRequest, SessionNotification, SessionUpdate};
use agent_client_protocol::{Dispatch, SessionMessage};
use futures_util::FutureExt as _;
use session_event_model::{InputId, SessionEvent, StopReason, TurnOutcome};

use crate::provider_item_projection::ProviderItemProjection;
use crate::provider_settings_catalog_codec::{
    apply_settings_update, catalog_from_session_response,
};
use crate::provider_update_kind::{is_known_update_kind, safe_update_kind};

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
                        result.map_err(acp_load_session_error).and_then(|restored| {
                            let mut session = restored.into_session();
                            let mut settings_catalog = catalog_from_session_response(&session.response());
                            let projection = replay_queued_session_updates(
                                &mut session,
                                Arc::clone(&event_sink),
                                &mut settings_catalog,
                            )?;
                            Ok(RestoredProviderSession {
                                session,
                                item_projection: Some(projection),
                                settings_catalog,
                            })
                        }),
                }
            }
            RestoreHistoryMode::WithoutReplay => {
                let request = ResumeSessionRequest::new(provider_session_id.clone(), &cwd)
                    .mcp_servers(mcp_servers);
                tokio::select! {
                    () = shutdown.cancelled() => Err(ExternalProviderRuntimeError::TransportFailure),
                    result = connection.resume_session_from(request).block_task().start_session() =>
                        result.map(|restored| {
                            let session = restored.into_session();
                            let settings_catalog = catalog_from_session_response(&session.response());
                            RestoredProviderSession { session, item_projection: None, settings_catalog }
                        }).map_err(acp_operation_error),
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

fn replay_queued_session_updates(
    session: &mut ActiveSession<'static, Agent>,
    event_sink: Arc<dyn SessionEventSink>,
    settings_catalog: &mut crate::ProviderSettingsCatalog,
) -> Result<ProviderItemProjection, ExternalProviderRuntimeError> {
    let session_id = session.session_id().to_string();
    let mut projection = ProviderItemProjection::new(session_id.clone(), Arc::clone(&event_sink));
    let mut historical_turn: Option<String> = None;
    let mut saw_agent_output = false;
    while let Some(update) = session.read_update().now_or_never() {
        let update = update.map_err(provider_frame_decode_error)?;
        let SessionMessage::SessionMessage(Dispatch::Notification(notification)) = update else {
            continue;
        };
        if notification.method() != "session/update" {
            continue;
        }
        let params = notification.params();
        let kind = params
            .get("update")
            .and_then(|update| update.get("sessionUpdate"))
            .and_then(serde_json::Value::as_str);
        if let Some(kind) = kind
            && !is_known_update_kind(kind)
        {
            let source_kind = safe_update_kind(kind).unwrap_or("unrecognized");
            let content = params
                .get("update")
                .and_then(|update| update.get("content"))
                .and_then(|content| content.get("text"))
                .and_then(serde_json::Value::as_str);
            projection
                .observe_unknown(source_kind, content)
                .map_err(|_| ExternalProviderRuntimeError::HistoryReplayUnavailable)?;
            continue;
        }
        let Ok(notification) = serde_json::from_value::<SessionNotification>(params.clone()) else {
            tracing::warn!("malformed ACP replay update");
            continue;
        };
        apply_settings_update(settings_catalog, &notification.update);
        let user_message = matches!(notification.update, SessionUpdate::UserMessageChunk(_));
        if user_message && (historical_turn.is_none() || saw_agent_output) {
            end_historical_turn(
                &mut historical_turn,
                &mut projection,
                &event_sink,
                &session_id,
            )?;
            let turn_id = uuid::Uuid::now_v7().to_string();
            event_sink
                .publish(
                    &session_id,
                    SessionEvent::TurnStarted {
                        turn_id: turn_id.clone(),
                        input_id: InputId::generate(),
                    },
                )
                .map_err(|_| ExternalProviderRuntimeError::HistoryReplayUnavailable)?;
            historical_turn = Some(turn_id);
            saw_agent_output = false;
        } else if !user_message {
            saw_agent_output = true;
        }
        projection
            .observe(&notification.update)
            .map_err(|_| ExternalProviderRuntimeError::HistoryReplayUnavailable)?;
    }
    end_historical_turn(
        &mut historical_turn,
        &mut projection,
        &event_sink,
        &session_id,
    )?;
    Ok(projection)
}

fn end_historical_turn(
    turn_id: &mut Option<String>,
    projection: &mut ProviderItemProjection,
    event_sink: &Arc<dyn SessionEventSink>,
    session_id: &str,
) -> Result<(), ExternalProviderRuntimeError> {
    projection
        .finish()
        .map_err(|_| ExternalProviderRuntimeError::HistoryReplayUnavailable)?;
    if let Some(turn_id) = turn_id.take() {
        event_sink
            .publish(
                session_id,
                SessionEvent::TurnEnded {
                    turn_id,
                    outcome: TurnOutcome::Ended {
                        stop_reason: StopReason::Unknown("replayed".to_owned()),
                        local_cause: None,
                    },
                },
            )
            .map_err(|_| ExternalProviderRuntimeError::HistoryReplayUnavailable)?;
    }
    Ok(())
}
