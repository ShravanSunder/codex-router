//! One provider session's prompt, cancellation, and steering actor.
use crate::InteractionPort;
use crate::SessionEventSink;
use crate::agent_session_client::CursorPlanItems;
#[cfg(any(test, feature = "test-observation"))]
use crate::agent_session_client::ExternalProviderToolCall;
use crate::agent_session_client::ProviderTurnCancellation;
use crate::agent_session_client::{
    ExternalProviderPromptOutcome, ExternalProviderRuntimeError, ProviderFrameObservation,
    acp_operation_error,
};
use crate::provider_connection_activity::ProviderConnectionActivity;
use crate::provider_item_projection::ProviderItemProjection;
use crate::provider_prompt_content::ProviderPromptContent;
use crate::provider_prompt_observation::{observe_idle_session_update, read_bounded_prompt};
use agent_client_protocol::schema::v1::{CancelNotification, PromptRequest};
use agent_client_protocol::{ActiveSession, Agent, ConnectionTo, JsonRpcMessage, UntypedMessage};
use serde_json::json;
use session_event_model::{InputId, LocalCause, SessionEvent, TurnOutcome};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const STEERING_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderSteeringOutcome<OperationId> {
    Injected {
        running_operation_id: Option<OperationId>,
    },
    PromptRequired,
    StartedNewTurn,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderPromptDispatchObservation {
    Submitted,
    NotSubmitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSessionActivity {
    NotLoaded,
    Idle,
    Running,
}

#[derive(Clone)]
pub(crate) struct ProviderSessionRuntimeHandles {
    pub(crate) event_sink: Arc<dyn SessionEventSink>,
    pub(crate) tool_registry: Arc<ProviderConnectionActivity>,
    pub(crate) todo_state: Arc<CursorPlanItems>,
    pub(crate) session_settings:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, crate::ProviderSettingsCatalog>>>,
    pub(crate) session_capabilities: Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, crate::ProviderCapabilityReport>>,
    >,
    pub(crate) last_settings_catalog:
        Arc<tokio::sync::RwLock<Option<crate::ProviderSettingsCatalog>>>,
    pub(crate) settings_unresolved:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, crate::ProviderSettingKind>>>,
}

pub(crate) enum ProviderSessionCommand<P: InteractionPort> {
    Prompt {
        input_id: InputId,
        operation_id: Option<P::OperationId>,
        prompt: ProviderPromptContent,
        turn_cancellation: Option<ProviderTurnCancellation<P>>,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
        reply: tokio::sync::oneshot::Sender<
            Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError>,
        >,
    },
    Cancel {
        expected_operation_id: Option<P::OperationId>,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    Steer {
        input_id: InputId,
        prompt: String,
        reply: tokio::sync::oneshot::Sender<
            Result<ProviderSteeringOutcome<P::OperationId>, ExternalProviderRuntimeError>,
        >,
    },
    Inspect {
        reply: tokio::sync::oneshot::Sender<ProviderSessionActivity>,
    },
    WaitIdle {
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    SetSetting {
        kind: crate::ProviderSettingKind,
        value: String,
        reply: tokio::sync::oneshot::Sender<
            Result<crate::EffectiveProviderSettings, ExternalProviderRuntimeError>,
        >,
    },
}

pub(crate) async fn run_provider_session<P: InteractionPort>(
    mut session: ActiveSession<'_, Agent>,
    mut commands: tokio::sync::mpsc::Receiver<ProviderSessionCommand<P>>,
    shutdown: CancellationToken,
    frame_observation: Arc<ProviderFrameObservation>,
    runtime_handles: ProviderSessionRuntimeHandles,
    #[cfg(any(test, feature = "test-observation"))] test_tool_calls: Arc<
        std::sync::Mutex<Vec<ExternalProviderToolCall>>,
    >,
) {
    let mut item_projection = ProviderItemProjection::new(
        session.session_id().to_string(),
        Arc::clone(&runtime_handles.event_sink),
    );
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            update = session.read_update() => {
                let Ok(update) = update else { break; };
                if observe_idle_session_update(
                    update,
                    &mut item_projection,
                    &runtime_handles,
                    session.session_id().0.as_ref(),
                ).await.is_err() {
                    break;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    ProviderSessionCommand::Prompt { input_id, operation_id, prompt, turn_cancellation, dispatch, reply } => {
                        let (terminal_tx, terminal_rx) = tokio::sync::oneshot::channel();
                        let prompt_request = PromptRequest::new(
                            session.session_id().clone(),
                            prompt.into_blocks(),
                        );
                        let prompt_request = match prompt_request.to_untyped_message() {
                            Ok(request) => request,
                            Err(error) => {
                                if let Some(dispatch) = dispatch {
                                    let _result = dispatch.send(ProviderPromptDispatchObservation::NotSubmitted);
                                }
                                let _result = reply.send(Err(acp_operation_error(error)));
                                continue;
                            }
                        };
                        if let Err(error) = session
                            .connection()
                            .send_request(prompt_request)
                            .on_receiving_result(async move |result| {
                                let _result = terminal_tx.send(result);
                                Ok(())
                            })
                        {
                            if let Some(dispatch) = dispatch {
                                let _result = dispatch.send(ProviderPromptDispatchObservation::NotSubmitted);
                            }
                            let _result = reply.send(Err(acp_operation_error(error)));
                            continue;
                        }
                        let turn_id = uuid::Uuid::now_v7().to_string();
                        runtime_handles.tool_registry.turn_started(session.session_id().0.as_ref(), &turn_id);
                        if runtime_handles.event_sink.publish(
                            session.session_id().0.as_ref(),
                            SessionEvent::TurnStarted { turn_id: turn_id.clone(), input_id },
                        ).is_err() {
                            let _result = session.connection().send_notification(
                                CancelNotification::new(session.session_id().clone()),
                            );
                            let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
                            runtime_handles.tool_registry.turn_ended(session.session_id().0.as_ref());
                            runtime_handles.todo_state.forget_session(session.session_id().0.as_ref());
                            return;
                        }
                        if let Some(dispatch) = dispatch {
                            let _result = dispatch.send(ProviderPromptDispatchObservation::Submitted);
                        }
                        let (output_limit_tx, mut output_limit_rx) =
                            tokio::sync::mpsc::unbounded_channel();
                        let provider_session_id = session.session_id().clone();
                        let provider_connection = session.connection().clone();
                        let mut idle_waiters = Vec::<tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>>::new();
                        let mut prompt_result = Box::pin(read_bounded_prompt(
                            &mut session,
                            terminal_rx,
                            output_limit_tx,
                            Arc::clone(&frame_observation),
                            &mut item_projection,
                            &runtime_handles,
                            #[cfg(any(test, feature = "test-observation"))]
                            Arc::clone(&test_tool_calls),
                        ));
                        let mut output_limit_cancelled = false;
                        loop {
                            tokio::select! {
                                biased;
                                () = shutdown.cancelled() => {
                                    if runtime_handles.tool_registry.claim_turn_end(provider_session_id.0.as_ref(), &turn_id) {
                                        let _result = runtime_handles.event_sink.publish(
                                            provider_session_id.0.as_ref(),
                                            SessionEvent::TurnEnded {
                                                turn_id: turn_id.clone(),
                                                outcome: TurnOutcome::Lost { reason: "providerRetired".to_owned() },
                                            },
                                        );
                                    }
                                    runtime_handles.todo_state.forget_session(provider_session_id.0.as_ref());
                                    let _result = reply.send(Err(ExternalProviderRuntimeError::Operation(
                                        "provider runtime shut down while prompt was active".to_owned(),
                                    )));
                                    return;
                                }
                                limit_notice = output_limit_rx.recv(), if !output_limit_cancelled => {
                                    if limit_notice.is_some() {
                                        output_limit_cancelled = true;
                                        if let Some(turn_cancellation) = &turn_cancellation {
                                            turn_cancellation.mark_cancelling();
                                        }
                                        let _result = provider_connection
                                            .send_notification(CancelNotification::new(provider_session_id.clone()));
                                        if let Some(turn_cancellation) = &turn_cancellation {
                                            turn_cancellation.settle_pending_approvals().await;
                                        }
                                    }
                                }
                                result = &mut prompt_result => {
                                    output_limit_cancelled |= runtime_handles
                                        .tool_registry
                                        .output_overflowed(provider_session_id.0.as_ref());
                                    let outcome = match &result {
                                        Ok(prompt) => TurnOutcome::Ended {
                                            stop_reason: prompt.stop_reason.clone(),
                                            local_cause: output_limit_cancelled.then_some(LocalCause::OutputOverflow),
                                        },
                                        Err(ExternalProviderRuntimeError::TransportFailure) => TurnOutcome::Lost { reason: "providerRetired".to_owned() },
                                        Err(_) => TurnOutcome::Lost { reason: "providerTurnFailed".to_owned() },
                                    };
                                    if runtime_handles.tool_registry.claim_turn_end(provider_session_id.0.as_ref(), &turn_id)
                                        && runtime_handles.event_sink.publish(
                                            provider_session_id.0.as_ref(),
                                            SessionEvent::TurnEnded { turn_id: turn_id.clone(), outcome },
                                        ).is_err()
                                    {
                                        runtime_handles.todo_state.forget_session(provider_session_id.0.as_ref());
                                        let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
                                        return;
                                    }
                                    runtime_handles.todo_state.forget_session(provider_session_id.0.as_ref());
                                    let reply_result = if output_limit_cancelled {
                                        Err(ExternalProviderRuntimeError::PromptOutputLimitExceeded)
                                    } else {
                                        result
                                    };
                                    let _result = reply.send(reply_result);
                                    for waiter in idle_waiters.drain(..) {
                                        let _result = waiter.send(Ok(()));
                                    }
                                    break;
                                }
                                command = commands.recv() => {
                                    match command {
                                        Some(ProviderSessionCommand::Cancel { expected_operation_id, reply }) => {
                                            let result = if expected_operation_id.is_some() && expected_operation_id != operation_id {
                                                Err(ExternalProviderRuntimeError::LocalCancelTargetMismatch)
                                            } else {
                                                provider_connection
                                                    .send_notification(CancelNotification::new(provider_session_id.clone()))
                                                    .map_err(acp_operation_error)
                                            };
                                            let _result = reply.send(result);
                                        }
                                        Some(ProviderSessionCommand::Prompt { reply, .. }) => {
                                            let _result = reply.send(Err(ExternalProviderRuntimeError::LocalBusy));
                                        }
                                        Some(ProviderSessionCommand::Steer { input_id, prompt, reply }) => {
                                            let result = steer_provider_turn::<P>(&provider_connection, &provider_session_id, prompt, operation_id.clone()).await;
                                            if matches!(result, Ok(ProviderSteeringOutcome::Injected { .. }))
                                                && runtime_handles.event_sink.publish(
                                                    provider_session_id.0.as_ref(),
                                                    SessionEvent::InputAccepted { input_id, turn_id: turn_id.clone() },
                                                ).is_err()
                                            {
                                                let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
                                                return;
                                            }
                                            let _result = reply.send(result);
                                        }
                                        Some(ProviderSessionCommand::Inspect { reply }) => {
                                            let _result = reply.send(ProviderSessionActivity::Running);
                                        }
                                        Some(ProviderSessionCommand::WaitIdle { reply }) => idle_waiters.push(reply),
                                        Some(ProviderSessionCommand::SetSetting { reply, .. }) => {
                                            let _result = reply.send(Err(ExternalProviderRuntimeError::LocalBusy));
                                        }
                                        None => return,
                                    }
                                }
                            }
                        }
                    }
                    ProviderSessionCommand::Cancel { reply, .. } => {
                        let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                    }
                    ProviderSessionCommand::Steer { input_id, prompt, reply } => {
                        let result = steer_provider_turn::<P>(session.connection(), session.session_id(), prompt, None).await;
                        if matches!(result, Ok(ProviderSteeringOutcome::StartedNewTurn)) {
                            let turn_id = uuid::Uuid::now_v7().to_string();
                            runtime_handles.tool_registry.turn_started(session.session_id().0.as_ref(), &turn_id);
                            if runtime_handles.event_sink.publish(
                                session.session_id().0.as_ref(),
                                SessionEvent::TurnStarted { turn_id: turn_id.clone(), input_id },
                            ).is_err() {
                                runtime_handles.tool_registry.turn_ended(session.session_id().0.as_ref());
                                runtime_handles.todo_state.forget_session(session.session_id().0.as_ref());
                                let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
                                return;
                            }
                            if runtime_handles.tool_registry.claim_turn_end(session.session_id().0.as_ref(), &turn_id)
                                && runtime_handles.event_sink.publish(
                                session.session_id().0.as_ref(),
                                SessionEvent::TurnEnded {
                                    turn_id,
                                    outcome: TurnOutcome::Lost { reason: "endNotObservable".to_owned() },
                                },
                            ).is_err() {
                                let _result = reply.send(Err(ExternalProviderRuntimeError::TransportFailure));
                                return;
                            }
                            runtime_handles.todo_state.forget_session(session.session_id().0.as_ref());
                        }
                        let _result = reply.send(result);
                    }
                    ProviderSessionCommand::Inspect { reply } => {
                        let _result = reply.send(ProviderSessionActivity::Idle);
                    }
                    ProviderSessionCommand::WaitIdle { reply } => {
                        let _result = reply.send(Ok(()));
                    }
                    ProviderSessionCommand::SetSetting { kind, value, reply } => {
                        let result = crate::provider_session_setting_update::apply_loaded_setting(
                            &session,
                            kind,
                            value,
                            &runtime_handles,
                        ).await;
                        let _result = reply.send(result);
                    }
                }
            }
        }
    }
}

async fn steer_provider_turn<P: InteractionPort>(
    connection: &ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::v1::SessionId,
    prompt: String,
    running_operation_id: Option<P::OperationId>,
) -> Result<ProviderSteeringOutcome<P::OperationId>, ExternalProviderRuntimeError> {
    let message = UntypedMessage::new(
        "_session/steering",
        json!({
            "sessionId": session_id.to_string(),
            "prompt": [{ "type": "text", "text": prompt }],
            "_meta": { "steering": { "idleBehavior": "promptRequired" } },
        }),
    )
    .map_err(acp_operation_error)?;
    let (reply, result) = tokio::sync::oneshot::channel();
    connection
        .send_request(message)
        .on_receiving_result(async move |response| {
            let _result = reply.send(response);
            Ok(())
        })
        .map_err(acp_operation_error)?;
    let response = tokio::time::timeout(STEERING_RESPONSE_TIMEOUT, result)
        .await
        .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
        .map_err(|_| ExternalProviderRuntimeError::TransportFailure)?
        .map_err(acp_operation_error)?;
    match response.get("outcome").and_then(serde_json::Value::as_str) {
        Some("injected") => Ok(ProviderSteeringOutcome::Injected {
            running_operation_id,
        }),
        Some("startedNewTurn") => Ok(ProviderSteeringOutcome::StartedNewTurn),
        Some("promptRequired") => Ok(ProviderSteeringOutcome::PromptRequired),
        Some("failed") => Ok(ProviderSteeringOutcome::Failed),
        _ => Err(ExternalProviderRuntimeError::Operation(
            "provider returned an invalid steering result".to_owned(),
        )),
    }
}
