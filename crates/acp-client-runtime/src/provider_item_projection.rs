//! Project streamed ACP updates into stable, ordered Session Items.

use std::{collections::HashMap, sync::Arc};

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionUpdate, ToolCall, ToolCallStatus as AcpToolCallStatus,
    ToolCallUpdate, ToolKind,
};
use session_event_model::{SessionEvent, SessionItem, SessionItemKind, ToolCallStatus};

use crate::{EventSinkOverflow, SessionEventSink, agent_session_client::MAX_PROMPT_OUTPUT_BYTES};

struct ActiveTextItem {
    item: SessionItem,
    message_id: Option<String>,
}

pub(crate) struct ProviderItemProjection {
    session_id: String,
    event_sink: Arc<dyn SessionEventSink>,
    active_text: Option<ActiveTextItem>,
    tool_items: HashMap<String, SessionItem>,
    tool_order: Vec<String>,
    plan_item: Option<SessionItem>,
}

impl ProviderItemProjection {
    pub(crate) fn new(session_id: String, event_sink: Arc<dyn SessionEventSink>) -> Self {
        Self {
            session_id,
            event_sink,
            active_text: None,
            tool_items: HashMap::new(),
            tool_order: Vec::new(),
            plan_item: None,
        }
    }

    pub(crate) fn observe(&mut self, update: &SessionUpdate) -> Result<(), EventSinkOverflow> {
        match update {
            SessionUpdate::UserMessageChunk(chunk) => {
                self.observe_text(chunk, SessionItemKind::UserMessage)
            }
            SessionUpdate::AgentMessageChunk(chunk) => {
                self.observe_text(chunk, SessionItemKind::AgentMessage)
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                self.observe_text(chunk, SessionItemKind::AgentThought)
            }
            SessionUpdate::ToolCall(tool_call) => {
                self.finish_text()?;
                self.observe_tool_call(tool_call)
            }
            SessionUpdate::ToolCallUpdate(update) => {
                self.finish_text()?;
                self.observe_tool_update(update)
            }
            SessionUpdate::Plan(plan) => {
                self.finish_text()?;
                self.observe_plan(plan)
            }
            SessionUpdate::CurrentModeUpdate(mode) => self.emit_once(
                SessionItemKind::ModeChange,
                mode.current_mode_id.0.to_string(),
            ),
            SessionUpdate::ConfigOptionUpdate(config) => self.emit_once(
                SessionItemKind::ConfigChange,
                serde_json::to_string(&config.config_options).unwrap_or_default(),
            ),
            SessionUpdate::SessionInfoUpdate(info) => self.emit_once(
                SessionItemKind::SessionInfo,
                serde_json::to_string(info).unwrap_or_default(),
            ),
            SessionUpdate::UsageUpdate(usage) => self.emit_once(
                SessionItemKind::Usage,
                serde_json::to_string(usage).unwrap_or_default(),
            ),
            SessionUpdate::AvailableCommandsUpdate(commands) => self.emit_once(
                SessionItemKind::Notice,
                serde_json::to_string(commands).unwrap_or_default(),
            ),
            _ => self.finish_text(),
        }
    }

    fn observe_text(
        &mut self,
        chunk: &ContentChunk,
        kind: SessionItemKind,
    ) -> Result<(), EventSinkOverflow> {
        let ContentBlock::Text(text) = &chunk.content else {
            self.finish_text()?;
            return Ok(());
        };
        let message_id = chunk.message_id.as_ref().map(|id| id.0.to_string());
        if self
            .active_text
            .as_ref()
            .is_some_and(|active| active.item.kind != kind || active.message_id != message_id)
        {
            self.finish_text()?;
        }
        if let Some(active) = &mut self.active_text {
            let prior = active.item.text.as_deref().unwrap_or_default();
            if prior.len().saturating_add(text.text.len()) > MAX_PROMPT_OUTPUT_BYTES {
                return Err(EventSinkOverflow);
            }
            active
                .item
                .text
                .get_or_insert_with(String::new)
                .push_str(&text.text);
            self.event_sink.publish(
                &self.session_id,
                SessionEvent::ItemUpdated {
                    item: active.item.clone(),
                },
            )
        } else {
            if text.text.len() > MAX_PROMPT_OUTPUT_BYTES {
                return Err(EventSinkOverflow);
            }
            let item = SessionItem {
                item_id: message_id
                    .clone()
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
                kind,
                text: Some(text.text.clone()),
            };
            self.event_sink.publish(
                &self.session_id,
                SessionEvent::ItemStarted { item: item.clone() },
            )?;
            self.active_text = Some(ActiveTextItem { item, message_id });
            Ok(())
        }
    }

    pub(crate) fn finish_text(&mut self) -> Result<(), EventSinkOverflow> {
        if let Some(active) = self.active_text.take() {
            self.event_sink.publish(
                &self.session_id,
                SessionEvent::ItemCompleted {
                    item_id: active.item.item_id,
                },
            )?;
        }
        Ok(())
    }

    fn observe_tool_call(&mut self, tool_call: &ToolCall) -> Result<(), EventSinkOverflow> {
        let item_id = tool_call.tool_call_id.0.to_string();
        let item = SessionItem {
            item_id: item_id.clone(),
            kind: SessionItemKind::ToolCall {
                tool_kind: tool_kind(tool_call.kind).to_owned(),
                status: tool_status(tool_call.status),
            },
            text: Some(tool_call.title.clone()),
        };
        let existed = self
            .tool_items
            .insert(item_id.clone(), item.clone())
            .is_some();
        if !existed {
            self.tool_order.push(item_id);
        }
        self.event_sink.publish(
            &self.session_id,
            if existed {
                SessionEvent::ItemUpdated { item }
            } else {
                SessionEvent::ItemStarted { item }
            },
        )
    }

    fn observe_tool_update(&mut self, update: &ToolCallUpdate) -> Result<(), EventSinkOverflow> {
        let item_id = update.tool_call_id.0.to_string();
        let existed = self.tool_items.contains_key(&item_id);
        let item = self
            .tool_items
            .entry(item_id.clone())
            .or_insert_with(|| SessionItem {
                item_id: item_id.clone(),
                kind: SessionItemKind::ToolCall {
                    tool_kind: "other".to_owned(),
                    status: ToolCallStatus::Pending,
                },
                text: None,
            });
        if let SessionItemKind::ToolCall {
            tool_kind: current_kind,
            status,
        } = &mut item.kind
        {
            if let Some(kind) = update.fields.kind {
                *current_kind = tool_kind(kind).to_owned();
            }
            if let Some(updated) = update.fields.status {
                *status = tool_status(updated);
            }
        }
        if let Some(title) = &update.fields.title {
            item.text = Some(title.clone());
        }
        let item = item.clone();
        if !existed {
            self.tool_order.push(item_id);
        }
        self.event_sink.publish(
            &self.session_id,
            if existed {
                SessionEvent::ItemUpdated { item }
            } else {
                SessionEvent::ItemStarted { item }
            },
        )
    }

    fn observe_plan(
        &mut self,
        plan: &agent_client_protocol::schema::v1::Plan,
    ) -> Result<(), EventSinkOverflow> {
        let text = plan
            .entries
            .iter()
            .map(|entry| format!("- {:?}: {}", entry.status, entry.content))
            .collect::<Vec<_>>()
            .join("\n");
        let existed = self.plan_item.is_some();
        let item = SessionItem {
            item_id: self.plan_item.as_ref().map_or_else(
                || uuid::Uuid::now_v7().to_string(),
                |item| item.item_id.clone(),
            ),
            kind: SessionItemKind::Plan,
            text: Some(text),
        };
        self.plan_item = Some(item.clone());
        self.event_sink.publish(
            &self.session_id,
            if existed {
                SessionEvent::ItemUpdated { item }
            } else {
                SessionEvent::ItemStarted { item }
            },
        )
    }

    pub(crate) fn finish(&mut self) -> Result<(), EventSinkOverflow> {
        self.finish_text()?;
        for item_id in self.tool_order.drain(..) {
            self.event_sink
                .publish(&self.session_id, SessionEvent::ItemCompleted { item_id })?;
        }
        if let Some(plan) = self.plan_item.take() {
            self.event_sink.publish(
                &self.session_id,
                SessionEvent::ItemCompleted {
                    item_id: plan.item_id,
                },
            )?;
        }
        Ok(())
    }

    pub(crate) fn observe_unknown(
        &mut self,
        source_kind: &str,
        content: Option<&str>,
    ) -> Result<(), EventSinkOverflow> {
        self.finish_text()?;
        let text = content.unwrap_or("Unrecognized agent update");
        if text.len() > MAX_PROMPT_OUTPUT_BYTES {
            return Err(EventSinkOverflow);
        }
        let item = SessionItem {
            item_id: uuid::Uuid::now_v7().to_string(),
            kind: SessionItemKind::Unknown {
                source_kind: source_kind.to_owned(),
            },
            text: Some(text.to_owned()),
        };
        self.event_sink.publish(
            &self.session_id,
            SessionEvent::ItemStarted { item: item.clone() },
        )?;
        self.event_sink.publish(
            &self.session_id,
            SessionEvent::ItemCompleted {
                item_id: item.item_id,
            },
        )
    }

    fn emit_once(&mut self, kind: SessionItemKind, text: String) -> Result<(), EventSinkOverflow> {
        self.finish_text()?;
        if text.len() > MAX_PROMPT_OUTPUT_BYTES {
            return Err(EventSinkOverflow);
        }
        let item = SessionItem {
            item_id: uuid::Uuid::now_v7().to_string(),
            kind,
            text: Some(text),
        };
        self.event_sink.publish(
            &self.session_id,
            SessionEvent::ItemStarted { item: item.clone() },
        )?;
        self.event_sink.publish(
            &self.session_id,
            SessionEvent::ItemCompleted {
                item_id: item.item_id,
            },
        )
    }
}

const fn tool_kind(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        _ => "other",
    }
}

const fn tool_status(status: AcpToolCallStatus) -> ToolCallStatus {
    match status {
        AcpToolCallStatus::Pending => ToolCallStatus::Pending,
        AcpToolCallStatus::InProgress => ToolCallStatus::InProgress,
        AcpToolCallStatus::Completed => ToolCallStatus::Completed,
        AcpToolCallStatus::Failed => ToolCallStatus::Failed,
        _ => ToolCallStatus::Pending,
    }
}
