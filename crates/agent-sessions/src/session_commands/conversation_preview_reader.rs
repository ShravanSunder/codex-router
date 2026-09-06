//! Bounded native rollout reads and sanitized human conversation snippets.

use serde_json::Value;
use std::fs;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;

use super::SESSION_CONVERSATION_MAX_READ_BYTES;
use super::SESSION_CONVERSATION_MAX_SNIPPETS;
use super::SESSION_CONVERSATION_SNIPPET_MAX_CHARS;
use super::truncate_end;

pub(super) fn read_history_tail(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    let file_len = metadata.len();
    let start = file_len.saturating_sub(SESSION_CONVERSATION_MAX_READ_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let tail_len = file_len - start;
    let mut bytes = Vec::with_capacity(tail_len as usize);
    file.take(SESSION_CONVERSATION_MAX_READ_BYTES)
        .read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    if start > 0
        && let Some((_, remaining)) = text.split_once('\n')
    {
        return Ok(remaining.to_owned());
    }
    Ok(text.into_owned())
}

pub(super) fn extract_recent_conversation_snippets(text: &str) -> Vec<String> {
    let mut recent_messages = Vec::new();
    let mut latest_user_message = None;
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(message) = conversation_message_from_event(&event) else {
            continue;
        };
        if message.is_user {
            latest_user_message = Some(message.clone());
        }
        recent_messages.push(message);
        if recent_messages.len() > SESSION_CONVERSATION_MAX_SNIPPETS {
            recent_messages.remove(0);
        }
    }
    if !recent_messages.iter().any(|message| message.is_user)
        && let Some(latest_user_message) = latest_user_message
    {
        if recent_messages.len() == SESSION_CONVERSATION_MAX_SNIPPETS {
            recent_messages.remove(0);
        }
        recent_messages.insert(0, latest_user_message);
    }
    recent_messages
        .into_iter()
        .map(|message| message.snippet)
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConversationMessage {
    snippet: String,
    is_user: bool,
}

fn conversation_message_from_event(event: &Value) -> Option<ConversationMessage> {
    if event.get("type").and_then(Value::as_str) != Some("response_item") {
        return None;
    }
    let payload = event.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let role = payload.get("role").and_then(Value::as_str)?;
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    let content = payload.get("content")?;
    let mut fragments = Vec::new();
    collect_text_fragments(content, &mut fragments);
    let text = fragments.join(" ");
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalized.trim();
    if normalized.is_empty() || is_control_conversation_text(normalized) {
        return None;
    }
    Some(ConversationMessage {
        snippet: truncate_end(normalized, SESSION_CONVERSATION_SNIPPET_MAX_CHARS),
        is_user: role == "user",
    })
}

fn collect_text_fragments(value: &Value, fragments: &mut Vec<String>) {
    match value {
        Value::String(text) => fragments.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_text_fragments(item, fragments);
            }
        }
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                fragments.push(text.to_owned());
                return;
            }
            for key in ["content", "output_text"] {
                if let Some(value) = object.get(key) {
                    collect_text_fragments(value, fragments);
                }
            }
        }
        _ => {}
    }
}

fn is_control_conversation_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    text.chars().count() > 5_000
        || lower.contains("agents.md instructions")
        || lower.contains("# agents.md")
        || lower.contains("<instructions>")
        || lower.contains("</instructions>")
        || lower.contains("<hook_prompt")
        || lower.contains("hook_run_id=")
        || lower.contains("<turn_aborted>")
        || lower.contains("<environment_context>")
        || lower.contains("<permissions instructions>")
        || lower.contains("<skills_instructions>")
        || lower.contains("<plugins_instructions>")
        || lower.contains("<subagent_notification>")
        || lower.contains("<user_instructions>")
        || lower.contains("<developer_instructions>")
        || lower.contains("<system_instructions>")
        || lower.contains("<context_summary>")
        || lower.contains("<tool_call>")
        || lower.contains("filesystem sandboxing")
        || lower.contains("available tools and usage guidelines")
        || lower.contains("the following is the codex agent history")
        || lower.contains("tool call arguments")
        || lower.contains(">>> transcript start")
        || lower.contains("transcript start")
        || lower.contains("transcript end")
        || lower.contains("review only p0-p2")
        || lower.contains("read-only implementation review")
        || lower.contains("scope: current uncommitted diff")
        || lower.contains("knowledge cutoff:")
        || lower.contains("you are codex")
        || lower.contains("available skills")
        || lower.contains("response_item")
        || lower.contains("api_key")
        || lower.contains("access_token")
        || lower.contains("refresh_token")
}
