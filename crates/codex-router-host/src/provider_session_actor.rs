//! One provider session's prompt, cancellation, and steering actor.
#[cfg(test)]
use crate::external_provider_runtime::ExternalProviderToolCall;
use crate::external_provider_runtime::{
    ExternalProviderPromptOutcome, ExternalProviderRuntimeError, acp_operation_error,
};
use crate::provider_prompt_observation::read_bounded_prompt;
use agent_client_protocol::schema::v1::{CancelNotification, PromptRequest};
use agent_client_protocol::{ActiveSession, Agent, ConnectionTo, UntypedMessage};
use collaboration_protocol::OperationId;
use serde_json::json;
#[cfg(test)]
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const STEERING_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderSteeringOutcome {
    Injected {
        running_operation_id: Option<OperationId>,
    },
    PromptRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderPromptDispatchObservation {
    Submitted,
    NotSubmitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSessionActivity {
    NotLoaded,
    Idle,
    Running,
}

pub(crate) enum ProviderSessionCommand {
    Prompt {
        operation_id: Option<OperationId>,
        prompt: String,
        dispatch: Option<tokio::sync::oneshot::Sender<ProviderPromptDispatchObservation>>,
        reply: tokio::sync::oneshot::Sender<
            Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError>,
        >,
    },
    Cancel {
        expected_operation_id: Option<OperationId>,
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
    Steer {
        prompt: String,
        reply: tokio::sync::oneshot::Sender<
            Result<ProviderSteeringOutcome, ExternalProviderRuntimeError>,
        >,
    },
    Inspect {
        reply: tokio::sync::oneshot::Sender<ProviderSessionActivity>,
    },
    WaitIdle {
        reply: tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>,
    },
}

pub(crate) async fn run_provider_session(
    mut session: ActiveSession<'_, Agent>,
    mut commands: tokio::sync::mpsc::Receiver<ProviderSessionCommand>,
    shutdown: CancellationToken,
    #[cfg(test)] test_tool_calls: Arc<std::sync::Mutex<Vec<ExternalProviderToolCall>>>,
) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    ProviderSessionCommand::Prompt { operation_id, prompt, dispatch, reply } => {
                        let (terminal_tx, terminal_rx) = tokio::sync::oneshot::channel();
                        let prompt_request = PromptRequest::new(
                            session.session_id().clone(),
                            vec![prompt.into()],
                        );
                        if let Err(error) = session
                            .connection()
                            .send_request(prompt_request)
                            .on_receiving_result(async move |result| {
                                let _result = terminal_tx.send(
                                    result.map(|response| response.stop_reason),
                                );
                                Ok(())
                            })
                        {
                            if let Some(dispatch) = dispatch {
                                let _result = dispatch.send(ProviderPromptDispatchObservation::NotSubmitted);
                            }
                            let _result = reply.send(Err(acp_operation_error(error)));
                            continue;
                        }
                        if let Some(dispatch) = dispatch {
                            let _result = dispatch.send(ProviderPromptDispatchObservation::Submitted);
                        }
                        let provider_session_id = session.session_id().clone();
                        let provider_connection = session.connection().clone();
                        let mut idle_waiters = Vec::<tokio::sync::oneshot::Sender<Result<(), ExternalProviderRuntimeError>>>::new();
                        let mut prompt_result = Box::pin(read_bounded_prompt(
                            &mut session,
                            terminal_rx,
                            #[cfg(test)]
                            Arc::clone(&test_tool_calls),
                        ));
                        loop {
                            tokio::select! {
                                () = shutdown.cancelled() => {
                                    let _result = reply.send(Err(ExternalProviderRuntimeError::Operation(
                                        "provider runtime shut down while prompt was active".to_owned(),
                                    )));
                                    return;
                                }
                                result = &mut prompt_result => {
                                    let _result = reply.send(result);
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
                                        Some(ProviderSessionCommand::Steer { prompt, reply }) => {
                                            let result = steer_provider_turn(&provider_connection, &provider_session_id, prompt, operation_id.clone()).await;
                                            let _result = reply.send(result);
                                        }
                                        Some(ProviderSessionCommand::Inspect { reply }) => {
                                            let _result = reply.send(ProviderSessionActivity::Running);
                                        }
                                        Some(ProviderSessionCommand::WaitIdle { reply }) => idle_waiters.push(reply),
                                        None => return,
                                    }
                                }
                            }
                        }
                    }
                    ProviderSessionCommand::Cancel { reply, .. } => {
                        let _result = reply.send(Err(ExternalProviderRuntimeError::LocalNotFound));
                    }
                    ProviderSessionCommand::Steer { prompt, reply } => {
                        let result = steer_provider_turn(session.connection(), session.session_id(), prompt, None).await;
                        let _result = reply.send(result);
                    }
                    ProviderSessionCommand::Inspect { reply } => {
                        let _result = reply.send(ProviderSessionActivity::Idle);
                    }
                    ProviderSessionCommand::WaitIdle { reply } => {
                        let _result = reply.send(Ok(()));
                    }
                }
            }
        }
    }
}

async fn steer_provider_turn(
    connection: &ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::v1::SessionId,
    prompt: String,
    running_operation_id: Option<OperationId>,
) -> Result<ProviderSteeringOutcome, ExternalProviderRuntimeError> {
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
        Some("promptRequired") => Ok(ProviderSteeringOutcome::PromptRequired),
        _ => Err(ExternalProviderRuntimeError::Operation(
            "provider returned an invalid steering result".to_owned(),
        )),
    }
}
