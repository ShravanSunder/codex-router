//! Classify ACP v1 update kinds before the SDK's closed enum is parsed.

use agent_client_protocol::Dispatch;

pub(crate) fn session_update_kind(dispatch: &Dispatch) -> Option<&str> {
    let Dispatch::Notification(message) = dispatch else {
        return None;
    };
    if message.method() != "session/update" {
        return None;
    }
    message
        .params()
        .get("update")?
        .get("sessionUpdate")?
        .as_str()
}

pub(crate) fn is_session_update_notification(dispatch: &Dispatch) -> bool {
    matches!(dispatch, Dispatch::Notification(message) if message.method() == "session/update")
}

pub(crate) fn is_known_update_kind(kind: &str) -> bool {
    matches!(
        kind,
        "user_message_chunk"
            | "agent_message_chunk"
            | "agent_thought_chunk"
            | "tool_call"
            | "tool_call_update"
            | "plan"
            | "available_commands_update"
            | "current_mode_update"
            | "config_option_update"
            | "session_info_update"
            | "usage_update"
    )
}

pub(crate) fn has_unknown_informational_value(dispatch: &Dispatch) -> bool {
    let Dispatch::Notification(message) = dispatch else {
        return false;
    };
    if message.method() != "session/update" {
        return false;
    }
    let Some(update) = message.params().get("update") else {
        return false;
    };
    match update
        .get("sessionUpdate")
        .and_then(serde_json::Value::as_str)
    {
        Some("tool_call" | "tool_call_update") => {
            let fields = update.get("fields").unwrap_or(update);
            fields
                .get("status")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|status| {
                    !matches!(status, "pending" | "in_progress" | "completed" | "failed")
                })
                || fields
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| {
                        !matches!(
                            kind,
                            "read"
                                | "edit"
                                | "delete"
                                | "move"
                                | "search"
                                | "execute"
                                | "think"
                                | "fetch"
                                | "switch_mode"
                                | "other"
                        )
                    })
        }
        Some("plan") => update
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|status| {
                            !matches!(status, "pending" | "in_progress" | "completed")
                        })
                })
            }),
        _ => false,
    }
}

pub(crate) fn safe_update_kind(kind: &str) -> Option<&str> {
    (!kind.is_empty()
        && kind.len() <= 32
        && kind
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_'))
    .then_some(kind)
}
