//! Bounded display titles, context and recency labels.
use super::{SESSION_CONTEXT_MAX_CHARS, SESSION_TITLE_MAX_CHARS, SessionRecord};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub(super) fn human_session_row(record: &SessionRecord) -> String {
    let context = record
        .cwd
        .as_deref()
        .map(session_context_from_cwd)
        .unwrap_or_else(|| "-".to_owned());
    format!(
        "{}\n  {}  {}  {}  id={}",
        record.display_title(),
        format_recency_at_ms(record.recency_at_ms),
        record.branch(),
        context,
        short_session_id(&record.session_id)
    )
}

pub(super) fn session_context_from_cwd(cwd: &str) -> String {
    let leaf = Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(cwd);
    truncate_middle(leaf, SESSION_CONTEXT_MAX_CHARS)
}

pub(super) fn display_title_from_session_fields(
    name: Option<&str>,
    title: Option<&str>,
    preview: Option<&str>,
    first_user_message: Option<&str>,
) -> Option<String> {
    let explicit_name = name.and_then(normalize_display_title);
    let derived_title = [title, preview, first_user_message]
        .into_iter()
        .flatten()
        .find_map(normalize_display_title);
    match (explicit_name, derived_title) {
        (Some(name), Some(derived_title)) => Some(truncate_end(
            &format!("{name} | {derived_title}"),
            SESSION_TITLE_MAX_CHARS,
        )),
        (Some(name), None) | (None, Some(name)) => Some(name),
        (None, None) => None,
    }
}

fn normalize_display_title(value: &str) -> Option<String> {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return None;
    }
    Some(truncate_end(&compact, SESSION_TITLE_MAX_CHARS))
}

pub(super) fn truncate_end(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}…", value.chars().take(keep).collect::<String>())
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    let side = max_chars.saturating_sub(1) / 2;
    let prefix = value.chars().take(side).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(side)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}…{suffix}")
}

fn short_session_id(session_id: &str) -> String {
    truncate_end(session_id, 8)
}

pub(super) fn format_recency_at_ms(recency_at_ms: Option<i64>) -> String {
    let Some(recency_at_ms) = recency_at_ms else {
        return "-".to_owned();
    };
    if recency_at_ms < 0 {
        return "-".to_owned();
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let recency_at_ms = recency_at_ms as u128;
    if now_ms >= recency_at_ms {
        let duration = format_duration_ms(now_ms - recency_at_ms);
        if duration == "now" {
            duration
        } else {
            format!("{duration} ago")
        }
    } else {
        let duration = format_duration_ms(recency_at_ms - now_ms);
        if duration == "now" {
            duration
        } else {
            format!("in {duration}")
        }
    }
}

pub(super) fn format_duration_ms(duration_ms: u128) -> String {
    let seconds = duration_ms / 1_000;
    if seconds < 60 {
        return "now".to_owned();
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    format!("{days}d")
}
