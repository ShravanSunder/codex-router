//! Bounded ACP prompt output and settlement observation.
use crate::agent_session_client::{
    ExternalProviderPromptOutcome, ExternalProviderRuntimeError, MAX_PROMPT_OUTPUT_BYTES,
    ProviderFrameObservation, acp_operation_error, provider_frame_decode_error,
};
#[cfg(any(test, feature = "test-observation"))]
use crate::agent_session_client::{
    ExternalProviderToolCall, ExternalProviderToolOutcome, classify_mcp_tool_outcome,
};
use crate::provider_item_projection::{ItemProjectionError, ProviderItemProjection};
use crate::provider_prompt_result_codec::{decode_prompt_result, decode_typed_stop_reason};
use crate::provider_session_actor::ProviderSessionRuntimeHandles;
use crate::provider_settings_catalog_codec::apply_settings_update;
use crate::provider_update_kind::{
    has_unknown_informational_value, is_known_update_kind, is_session_update_notification,
    safe_update_kind, session_update_kind,
};
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionNotification, SessionUpdate, StopReason,
};
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::{ActiveSession, Agent, Dispatch, Error, SessionMessage};
use session_event_model::SessionEvent;
use std::collections::HashSet;
#[cfg(any(test, feature = "test-observation"))]
use std::{collections::HashMap, sync::Arc};

struct SessionUpdateContext<'a> {
    item_projection: &'a mut ProviderItemProjection,
    runtime_handles: &'a ProviderSessionRuntimeHandles,
    session_id: &'a str,
}

pub(crate) async fn read_bounded_prompt(
    session: &mut ActiveSession<'_, Agent>,
    mut terminal: tokio::sync::oneshot::Receiver<
        Result<serde_json::Value, agent_client_protocol::Error>,
    >,
    output_limit_tx: tokio::sync::mpsc::UnboundedSender<()>,
    frame_observation: std::sync::Arc<ProviderFrameObservation>,
    item_projection: &mut ProviderItemProjection,
    runtime_handles: &ProviderSessionRuntimeHandles,
    #[cfg(any(test, feature = "test-observation"))] test_tool_calls: Arc<
        std::sync::Mutex<Vec<ExternalProviderToolCall>>,
    >,
) -> Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError> {
    use futures_util::FutureExt as _;

    let session_id = session.session_id().to_string();
    let mut output = String::new();
    let mut unknown_update_kinds = HashSet::<String>::new();
    #[cfg(any(test, feature = "test-observation"))]
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
                    &mut unknown_update_kinds,
                    &mut SessionUpdateContext { item_projection, runtime_handles, session_id: &session_id },
                    #[cfg(any(test, feature = "test-observation"))] &mut tool_calls,
                    #[cfg(any(test, feature = "test-observation"))] &test_tool_calls,
                ).await? {
                    break decode_typed_stop_reason(reason)?;
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
                let reason = decode_prompt_result(&result.map_err(acp_operation_error)?)?;
                while let Some(update) = session.read_update().now_or_never() {
                    let update = update.map_err(provider_frame_decode_error)?;
                    let _legacy_reason = record_prompt_update(
                        update,
                        &mut output,
                        &mut output_limit_tx,
                        &mut unknown_update_kinds,
                        &mut SessionUpdateContext { item_projection, runtime_handles, session_id: &session_id },
                        #[cfg(any(test, feature = "test-observation"))] &mut tool_calls,
                        #[cfg(any(test, feature = "test-observation"))] &test_tool_calls,
                    ).await?;
                }
                break reason;
            }
        }
    };
    item_projection.finish().map_err(projection_error)?;
    #[cfg(any(test, feature = "test-observation"))]
    {
        *test_tool_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            tool_calls.into_values().collect();
    }
    Ok(ExternalProviderPromptOutcome {
        output,
        stop_reason,
        permission_refusal_reason: None,
    })
}

/// The Session actor calls this while idle. Notifications still update the
/// same projection that the next prompt will use; only requests are refused.
pub(crate) async fn observe_idle_session_update(
    message: SessionMessage,
    item_projection: &mut ProviderItemProjection,
    runtime_handles: &ProviderSessionRuntimeHandles,
    session_id: &str,
) -> Result<(), ExternalProviderRuntimeError> {
    let SessionMessage::SessionMessage(dispatch) = message else {
        return Ok(());
    };
    match dispatch {
        Dispatch::Request(_, responder) => responder
            .respond_with_error(Error::method_not_found())
            .map_err(acp_operation_error),
        Dispatch::Notification(notification) if notification.method() == "session/update" => {
            let params = notification.params();
            let update = params.get("update");
            let kind = update
                .and_then(|update| update.get("sessionUpdate"))
                .and_then(serde_json::Value::as_str);
            let unknown = kind.is_some_and(|kind| !is_known_update_kind(kind))
                || has_unknown_informational_value(&Dispatch::Notification(notification.clone()));
            if unknown {
                let source_kind = kind.and_then(safe_update_kind).unwrap_or("unrecognized");
                let content = update
                    .and_then(|update| update.get("content"))
                    .and_then(|content| content.get("text"))
                    .and_then(serde_json::Value::as_str);
                item_projection
                    .observe_unknown(source_kind, content)
                    .map_err(projection_error)?;
                return Ok(());
            }
            let parsed = serde_json::from_value::<SessionNotification>(params.clone());
            let Ok(parsed) = parsed else {
                tracing::warn!("malformed idle ACP session update");
                return Ok(());
            };
            update_live_settings(&parsed.update, runtime_handles, session_id)
                .await
                .map_err(|_| ExternalProviderRuntimeError::SinkClosed)?;
            item_projection
                .observe(&parsed.update)
                .map_err(projection_error)
        }
        Dispatch::Notification(_) | Dispatch::Response(_, _) => Ok(()),
    }
}

fn projection_error(error: ItemProjectionError) -> ExternalProviderRuntimeError {
    match error {
        ItemProjectionError::OutputLimit => ExternalProviderRuntimeError::PromptOutputLimitExceeded,
        ItemProjectionError::SinkClosed => ExternalProviderRuntimeError::SinkClosed,
    }
}

async fn update_live_settings(
    update: &SessionUpdate,
    handles: &ProviderSessionRuntimeHandles,
    session_id: &str,
) -> Result<(), crate::EventSinkClosed> {
    let capability_change = match update {
        SessionUpdate::CurrentModeUpdate(_) => Some(true),
        SessionUpdate::ConfigOptionUpdate(_) => Some(false),
        _ => None,
    };
    let catalog = if capability_change.is_some() {
        let mut catalogs = handles.session_settings.write().await;
        let catalog = catalogs.entry(session_id.to_owned()).or_default();
        apply_settings_update(catalog, update);
        Some(catalog.clone())
    } else {
        None
    };
    if let Some(catalog) = catalog {
        handles.event_sink.publish(
            session_id,
            SessionEvent::SettingsChanged {
                settings: catalog.to_session_settings(),
            },
        )?;
        *handles.last_settings_catalog.write().await = Some(catalog);
    }
    if let Some(modes_update) = capability_change {
        let capabilities = {
            let mut reports = handles.session_capabilities.write().await;
            reports.get_mut(session_id).map(|report| {
                let flag = if modes_update {
                    &mut report.supports_modes
                } else {
                    &mut report.supports_config_options
                };
                *flag = true;
                report.to_session_model()
            })
        };
        if let Some(capabilities) = capabilities {
            handles.event_sink.publish(
                session_id,
                SessionEvent::CapabilitiesChanged { capabilities },
            )?;
        }
    }
    Ok(())
}

async fn record_prompt_update(
    update: SessionMessage,
    output: &mut String,
    output_limit_tx: &mut Option<tokio::sync::mpsc::UnboundedSender<()>>,
    unknown_update_kinds: &mut HashSet<String>,
    session_context: &mut SessionUpdateContext<'_>,
    #[cfg(any(test, feature = "test-observation"))] tool_calls: &mut HashMap<
        String,
        ExternalProviderToolCall,
    >,
    #[cfg(any(test, feature = "test-observation"))] test_tool_calls: &Arc<
        std::sync::Mutex<Vec<ExternalProviderToolCall>>,
    >,
) -> Result<Option<StopReason>, ExternalProviderRuntimeError> {
    match update {
        SessionMessage::SessionMessage(dispatch) => {
            let is_session_update = is_session_update_notification(&dispatch);
            let update_kind = session_update_kind(&dispatch).map(str::to_owned);
            if let Some(kind) = update_kind.as_deref()
                && (!is_known_update_kind(kind) || has_unknown_informational_value(&dispatch))
            {
                let diagnostic_kind = safe_update_kind(kind).unwrap_or("unrecognized");
                if unknown_update_kinds.insert(diagnostic_kind.to_owned()) {
                    tracing::warn!(
                        update_kind = diagnostic_kind,
                        "unknown ACP session update kind"
                    );
                }
                let content = match &dispatch {
                    agent_client_protocol::Dispatch::Notification(notification) => notification
                        .params()
                        .get("update")
                        .and_then(|update| update.get("content"))
                        .and_then(|content| content.get("text"))
                        .and_then(serde_json::Value::as_str),
                    _ => None,
                };
                if let Err(error) = session_context
                    .item_projection
                    .observe_unknown(diagnostic_kind, content)
                {
                    match error {
                        ItemProjectionError::OutputLimit => {
                            if let Some(limit_tx) = output_limit_tx.take() {
                                let _result = limit_tx.send(());
                            }
                        }
                        ItemProjectionError::SinkClosed => {
                            return Err(ExternalProviderRuntimeError::SinkClosed);
                        }
                    }
                }
                return Ok(None);
            }
            let mut sink_closed = false;
            let handled = MatchDispatch::new(dispatch)
                .if_notification(async |notification: SessionNotification| {
                    if update_live_settings(
                        &notification.update,
                        session_context.runtime_handles,
                        session_context.session_id,
                    )
                    .await
                    .is_err()
                    {
                        sink_closed = true;
                    }
                    if let Err(error) = session_context
                        .item_projection
                        .observe(&notification.update)
                    {
                        match error {
                            ItemProjectionError::OutputLimit => {
                                if let Some(limit_tx) = output_limit_tx.take() {
                                    let _result = limit_tx.send(());
                                }
                            }
                            ItemProjectionError::SinkClosed => sink_closed = true,
                        }
                    }
                    match notification.update {
                        SessionUpdate::AgentMessageChunk(ContentChunk {
                            content: ContentBlock::Text(text),
                            ..
                        }) => {
                            if output.len().saturating_add(text.text.len())
                                > MAX_PROMPT_OUTPUT_BYTES
                            {
                                if let Some(limit_tx) = output_limit_tx.take() {
                                    let _result = limit_tx.send(());
                                }
                            } else {
                                output.push_str(&text.text);
                            }
                        }
                        SessionUpdate::ToolCall(tool_call) => {
                            #[cfg(not(any(test, feature = "test-observation")))]
                            let _ = tool_call;
                            #[cfg(any(test, feature = "test-observation"))]
                            tool_calls.insert(
                                tool_call.tool_call_id.0.to_string(),
                                ExternalProviderToolCall {
                                    name: tool_call.name,
                                    title: tool_call.title,
                                    kind: tool_call.kind,
                                    status: tool_call.status,
                                    outcome: classify_mcp_tool_outcome(
                                        tool_call.raw_output.as_ref(),
                                    ),
                                },
                            );
                        }
                        SessionUpdate::ToolCallUpdate(update) => {
                            #[cfg(not(any(test, feature = "test-observation")))]
                            let _ = update;
                            #[cfg(any(test, feature = "test-observation"))]
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
                    #[cfg(any(test, feature = "test-observation"))]
                    {
                        *test_tool_calls
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
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
                .await;
            if sink_closed {
                return Err(ExternalProviderRuntimeError::SinkClosed);
            }
            if handled.is_err() && is_session_update {
                tracing::warn!(
                    update_kind = update_kind
                        .as_deref()
                        .and_then(safe_update_kind)
                        .unwrap_or("unrecognized"),
                    "malformed ACP session update"
                );
                return Ok(None);
            }
            handled.map_err(provider_frame_decode_error)?;
        }
        SessionMessage::StopReason(reason) => return Ok(Some(reason)),
        _ => {}
    }
    Ok(None)
}
