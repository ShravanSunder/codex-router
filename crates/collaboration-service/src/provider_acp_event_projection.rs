//! Projects shared provider Session events onto one ACP client connection.
use crate::{
    SessionEventHub,
    provider_acp_session_route::{failure, response},
};
use message_board::SessionRef;
use serde_json::{Value, json};
use session_event_model::session_profile_codec::{ProfileState, StateNotification};
use session_event_model::{
    CapabilityReport, PendingInteraction, SessionEvent, SessionItemKind, SessionState, StopReason,
    ToolCallStatus, TurnOutcome,
};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

fn capabilities_from_snapshot(snapshot: &[crate::HubEvent]) -> Option<CapabilityReport> {
    snapshot
        .iter()
        .filter_map(|event| match &event.event {
            SessionEvent::CapabilitiesChanged { capabilities } => Some(capabilities.clone()),
            _ => None,
        })
        .next_back()
}

fn event_notification(
    event: &SessionEvent,
    session: &SessionRef,
    supports_state: bool,
    item_text: &mut HashMap<String, String>,
) -> Option<Value> {
    match event {
        SessionEvent::StateChanged { state } if supports_state => {
            let state = match state {
                SessionState::Running => Some(ProfileState::Running),
                SessionState::Idle => Some(ProfileState::Idle),
                SessionState::RequiresAction { pending } => Some(ProfileState::RequiresAction {
                    kind: pending.first().kind(),
                }),
                _ => None,
            }?;
            let notification = StateNotification {
                session_id: session.session_id.as_str().to_owned(),
                state,
            };
            Some(json!({"jsonrpc":"2.0","method":notification.method(),"params":notification}))
        }
        SessionEvent::ItemStarted { item } | SessionEvent::ItemUpdated { item }
            if matches!(item.kind, SessionItemKind::ToolCall { .. }) =>
        {
            let SessionItemKind::ToolCall { tool_kind, status } = &item.kind else {
                return None;
            };
            let kind = match tool_kind.as_str() {
                "read" | "edit" | "delete" | "move" | "search" | "execute" | "think" | "fetch"
                | "switch_mode" => tool_kind.as_str(),
                _ => "other",
            };
            let status = match status {
                ToolCallStatus::Pending => "pending",
                ToolCallStatus::InProgress => "in_progress",
                ToolCallStatus::Completed => "completed",
                ToolCallStatus::Failed => "failed",
            };
            let started = matches!(event, SessionEvent::ItemStarted { .. });
            let update = if started {
                json!({"sessionUpdate":"tool_call","toolCallId":item.item_id,
                    "kind":kind,"status":status,
                    "title":item.text.as_deref().unwrap_or(tool_kind)})
            } else {
                json!({"sessionUpdate":"tool_call_update","toolCallId":item.item_id,
                    "status":status,"title":item.text})
            };
            Some(json!({"jsonrpc":"2.0","method":"session/update","params":{
                "sessionId":session.session_id.as_str(),"update":update
            }}))
        }
        SessionEvent::ItemStarted { item } | SessionEvent::ItemUpdated { item } => {
            let text = item.text.as_deref()?;
            let previous = item_text.insert(item.item_id.clone(), text.to_owned());
            let delta = previous
                .as_ref()
                .and_then(|before| text.strip_prefix(before))
                .unwrap_or(text);
            if delta.is_empty() {
                return None;
            }
            let update_kind = match item.kind {
                SessionItemKind::AgentThought => "agent_thought_chunk",
                SessionItemKind::UserMessage => "user_message_chunk",
                _ => "agent_message_chunk",
            };
            Some(json!({"jsonrpc":"2.0","method":"session/update","params":{
                "sessionId":session.session_id.as_str(),
                "update":{"sessionUpdate":update_kind,"content":{"type":"text","text":delta}}
            }}))
        }
        SessionEvent::TurnEnded {
            outcome: TurnOutcome::Lost { reason },
            ..
        } if supports_state => {
            let notification = StateNotification {
                session_id: session.session_id.as_str().to_owned(),
                state: ProfileState::Lost {
                    reason: reason.clone(),
                    stop_reason: None,
                },
            };
            Some(json!({"jsonrpc":"2.0","method":notification.method(),"params":notification}))
        }
        _ => None,
    }
}

pub(crate) struct ProviderSessionObservers {
    events: Arc<dyn SessionEventHub>,
    output: codex_acp_adapter::AcpOutputSender,
    supports_state: bool,
    interaction_sender: mpsc::Sender<(SessionRef, PendingInteraction)>,
    pub(crate) tasks: JoinSet<()>,
    attached: BTreeSet<SessionRef>,
    pub(crate) observed_turns: HashMap<SessionRef, watch::Receiver<Option<String>>>,
    cancellations: HashMap<SessionRef, CancellationToken>,
}

impl ProviderSessionObservers {
    pub(crate) fn new(
        events: Arc<dyn SessionEventHub>,
        output: codex_acp_adapter::AcpOutputSender,
        supports_state: bool,
        interaction_sender: mpsc::Sender<(SessionRef, PendingInteraction)>,
    ) -> Self {
        Self {
            events,
            output,
            supports_state,
            interaction_sender,
            tasks: JoinSet::new(),
            attached: BTreeSet::new(),
            observed_turns: HashMap::new(),
            cancellations: HashMap::new(),
        }
    }

    pub(crate) async fn attach(
        &mut self,
        session: SessionRef,
        replay_history: bool,
        replace_existing: bool,
    ) -> Result<CapabilityReport, ()> {
        if replace_existing {
            if let Some(previous) = self.cancellations.remove(&session) {
                previous.cancel();
            }
            self.attached.remove(&session);
            self.observed_turns.remove(&session);
        }
        let attachment = self.events.attach(session.clone()).await.map_err(|_| ())?;
        let report = capabilities_from_snapshot(&attachment.snapshot).ok_or(())?;
        if self.attached.insert(session.clone()) {
            let (turn_sender, turn_receiver) = watch::channel(None);
            self.observed_turns.insert(session.clone(), turn_receiver);
            let cancellation = CancellationToken::new();
            self.cancellations
                .insert(session.clone(), cancellation.clone());
            let mut item_text = HashMap::new();
            for event in &attachment.snapshot {
                if let SessionEvent::InteractionRequested { interaction } = &event.event {
                    self.interaction_sender
                        .send((session.clone(), interaction.clone()))
                        .await
                        .map_err(|_| ())?;
                }
            }
            if replay_history {
                for event in &attachment.snapshot {
                    if let Some(notification) = event_notification(
                        &event.event,
                        &session,
                        self.supports_state,
                        &mut item_text,
                    ) {
                        self.output.send(notification).await.map_err(|_| ())?;
                    }
                }
            }
            let mut receiver = attachment.receiver;
            let output = self.output.clone();
            let events = Arc::clone(&self.events);
            let supports_state = self.supports_state;
            let interaction_sender = self.interaction_sender.clone();
            self.tasks.spawn(async move {
            loop {
                let received = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => break,
                    received = receiver.recv() => received,
                };
                let event = match received {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if supports_state {
                            let notification = StateNotification {
                                session_id: session.session_id.as_str().to_owned(),
                                state: ProfileState::Lost { reason: "event stream lagged; reattach required".into(), stop_reason: None },
                            };
                            let _sent = output.send(json!({"jsonrpc":"2.0","method":notification.method(),"params":notification})).await;
                        }
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if cancellation.is_cancelled() { break; }
                if matches!(event.event, SessionEvent::ResyncRequired { .. }) {
                    let Ok(replacement) = events.attach(session.clone()).await else { break; };
                    item_text.clear();
                    for historical in &replacement.snapshot {
                        if let SessionEvent::InteractionRequested { interaction } = &historical.event
                            && interaction_sender.send((session.clone(), interaction.clone())).await.is_err() { return; }
                        if let Some(notification) = event_notification(&historical.event, &session, supports_state, &mut item_text)
                            && output.send(notification).await.is_err() { return; }
                    }
                    receiver = replacement.receiver;
                    continue;
                }
                if let SessionEvent::InteractionRequested { interaction } = &event.event
                    && interaction_sender.send((session.clone(), interaction.clone())).await.is_err() { break; }
                if let Some(notification) = event_notification(&event.event, &session, supports_state, &mut item_text)
                    && output.send(notification).await.is_err() { break; }
                if let SessionEvent::TurnEnded { turn_id, .. } = event.event {
                    let _sent = turn_sender.send(Some(turn_id));
                }
            }
        });
        }
        Ok(report)
    }
}

pub(crate) async fn stream_prompt(
    output: codex_acp_adapter::AcpOutputSender,
    id: Value,
    turn_id: String,
    mut attachment: crate::SessionEventAttachment,
    mut observed_turn: watch::Receiver<Option<String>>,
) {
    let mut replay = attachment.snapshot.into_iter();
    loop {
        let event = match replay.next() {
            Some(event) => event,
            None => match attachment.receiver.recv().await {
                Ok(event) => event,
                Err(_) => {
                    let _sent = output
                        .send(failure(id, -32000, "Session event stream unavailable"))
                        .await;
                    return;
                }
            },
        };
        match event.event {
            SessionEvent::TurnEnded {
                turn_id: ended,
                outcome,
            } if ended == turn_id => {
                while observed_turn.borrow().as_deref() != Some(ended.as_str()) {
                    if observed_turn.changed().await.is_err() {
                        let _sent = output
                            .send(failure(id, -32000, "Session observer unavailable"))
                            .await;
                        return;
                    }
                }
                let stop_reason = match outcome {
                    TurnOutcome::Ended { stop_reason, .. } => match stop_reason {
                        StopReason::EndTurn => "end_turn",
                        StopReason::MaxTokens => "max_tokens",
                        StopReason::MaxTurnRequests => "max_turn_requests",
                        StopReason::Refusal => "refusal",
                        StopReason::Cancelled => "cancelled",
                        StopReason::Unknown(_) => "end_turn",
                    },
                    TurnOutcome::Lost { .. } => {
                        let _sent = output.send(failure(id, -32000, "Session turn lost")).await;
                        return;
                    }
                };
                let _sent = output
                    .send(response(id, json!({"stopReason":stop_reason})))
                    .await;
                return;
            }
            _ => {}
        }
    }
}
