//! Bounded ACP prompt output and settlement observation.
use crate::external_provider_runtime::{
    ExternalProviderPromptOutcome, ExternalProviderRuntimeError, MAX_PROMPT_OUTPUT_BYTES,
    ProviderFrameObservation, acp_operation_error, provider_frame_decode_error,
};
#[cfg(test)]
use crate::external_provider_runtime::{
    ExternalProviderToolCall, ExternalProviderToolOutcome, classify_mcp_tool_outcome,
};
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionNotification, SessionUpdate, StopReason,
};
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::{ActiveSession, Agent, SessionMessage};
use collaboration_protocol::ProviderPromptStopReason;
#[cfg(test)]
use std::{collections::HashMap, sync::Arc};

pub(crate) async fn read_bounded_prompt(
    session: &mut ActiveSession<'_, Agent>,
    mut terminal: tokio::sync::oneshot::Receiver<Result<StopReason, agent_client_protocol::Error>>,
    output_limit_tx: tokio::sync::mpsc::UnboundedSender<()>,
    frame_observation: std::sync::Arc<ProviderFrameObservation>,
    #[cfg(test)] test_tool_calls: Arc<std::sync::Mutex<Vec<ExternalProviderToolCall>>>,
) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
    use futures_util::FutureExt as _;

    let mut output = String::new();
    #[cfg(test)]
    let mut tool_calls = HashMap::<String, ExternalProviderToolCall>::new();
    let mut output_limit_tx = Some(output_limit_tx);
    let stop_reason = loop {
        tokio::select! {
            biased;
            update = session.read_update() => {
                let update = update.map_err(provider_frame_decode_error)?;
                if let Some(reason) = record_prompt_update(
                    update,
                    &mut output,
                    &mut output_limit_tx,
                    #[cfg(test)] &mut tool_calls,
                    #[cfg(test)] &test_tool_calls,
                ).await? {
                    break reason;
                }
            }
            result = &mut terminal => {
                let result = match result {
                    Ok(result) => result,
                    Err(_) => {
                        if !frame_observation.limit_was_exceeded() {
                            let frame_limit_observed =
                                frame_observation.wait_for_limit_exceeded().await;
                            if !frame_limit_observed {
                                return Err(ExternalProviderRuntimeError::TransportFailure);
                            }
                        }
                        return Err(ExternalProviderRuntimeError::FrameLimitExceeded);
                    }
                };
                let reason = result.map_err(acp_operation_error)?;
                while let Some(update) = session.read_update().now_or_never() {
                    let update = update.map_err(provider_frame_decode_error)?;
                    let _legacy_reason = record_prompt_update(
                        update,
                        &mut output,
                        &mut output_limit_tx,
                        #[cfg(test)] &mut tool_calls,
                        #[cfg(test)] &test_tool_calls,
                    ).await?;
                }
                break reason;
            }
        }
    };
    #[cfg(test)]
    {
        *test_tool_calls.lock().expect("test tool calls") = tool_calls.into_values().collect();
    }
    Ok(ExternalProviderPromptOutcome {
        output,
        stop_reason: provider_prompt_stop_reason(stop_reason)?,
    })
}

async fn record_prompt_update(
    update: SessionMessage,
    output: &mut String,
    output_limit_tx: &mut Option<tokio::sync::mpsc::UnboundedSender<()>>,
    #[cfg(test)] tool_calls: &mut HashMap<String, ExternalProviderToolCall>,
    #[cfg(test)] test_tool_calls: &Arc<std::sync::Mutex<Vec<ExternalProviderToolCall>>>,
) -> Result<Option<StopReason>, ExternalProviderRuntimeError> {
    match update {
        SessionMessage::SessionMessage(dispatch) => MatchDispatch::new(dispatch)
            .if_notification(async |notification: SessionNotification| {
                match notification.update {
                    SessionUpdate::AgentMessageChunk(ContentChunk {
                        content: ContentBlock::Text(text),
                        ..
                    }) => {
                        if output.len().saturating_add(text.text.len()) > MAX_PROMPT_OUTPUT_BYTES {
                            if let Some(limit_tx) = output_limit_tx.take() {
                                let _result = limit_tx.send(());
                            }
                        } else {
                            output.push_str(&text.text);
                        }
                    }
                    SessionUpdate::ToolCall(tool_call) => {
                        #[cfg(not(test))]
                        let _ = tool_call;
                        #[cfg(test)]
                        tool_calls.insert(
                            tool_call.tool_call_id.0.to_string(),
                            ExternalProviderToolCall {
                                name: tool_call.name,
                                title: tool_call.title,
                                kind: tool_call.kind,
                                status: tool_call.status,
                                outcome: classify_mcp_tool_outcome(tool_call.raw_output.as_ref()),
                            },
                        );
                    }
                    SessionUpdate::ToolCallUpdate(update) => {
                        #[cfg(not(test))]
                        let _ = update;
                        #[cfg(test)]
                        if let Some(observation) =
                            tool_calls.get_mut(update.tool_call_id.0.as_ref())
                        {
                            if let Some(name) = update.fields.name {
                                observation.name = Some(name);
                            }
                            if let Some(title) = update.fields.title {
                                observation.title = title;
                            }
                            if let Some(kind) = update.fields.kind {
                                observation.kind = kind;
                            }
                            if let Some(status) = update.fields.status {
                                observation.status = status;
                            }
                            let outcome =
                                classify_mcp_tool_outcome(update.fields.raw_output.as_ref());
                            if outcome != ExternalProviderToolOutcome::Unknown {
                                observation.outcome = outcome;
                            }
                        }
                    }
                    _ => {}
                }
                #[cfg(test)]
                {
                    *test_tool_calls.lock().expect("test tool calls") =
                        tool_calls.values().cloned().collect();
                }
                Ok(())
            })
            .await
            .otherwise(|message| async move {
                match message {
                    agent_client_protocol::Dispatch::Request(_, responder) => responder
                        .respond_with_error(agent_client_protocol::Error::method_not_found()),
                    agent_client_protocol::Dispatch::Notification(_)
                    | agent_client_protocol::Dispatch::Response(_, _) => Ok(()),
                }
            })
            .await
            .map_err(provider_frame_decode_error)?,
        SessionMessage::StopReason(reason) => return Ok(Some(reason)),
        _ => {}
    }
    Ok(None)
}

fn provider_prompt_stop_reason(
    reason: StopReason,
) -> Result<ProviderPromptStopReason, ExternalProviderRuntimeError> {
    match reason {
        StopReason::EndTurn => Ok(ProviderPromptStopReason::EndTurn),
        StopReason::MaxTokens => Ok(ProviderPromptStopReason::MaxTokens),
        StopReason::MaxTurnRequests => Ok(ProviderPromptStopReason::MaxTurnRequests),
        StopReason::Refusal => Ok(ProviderPromptStopReason::Refusal),
        StopReason::Cancelled => Ok(ProviderPromptStopReason::Cancelled),
        _ => Err(ExternalProviderRuntimeError::Operation(
            "provider returned an unsupported prompt stop reason".to_owned(),
        )),
    }
}
