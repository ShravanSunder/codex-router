//! Picker display text.
use super::{SessionsPickerRoot, SessionsSort};
use crate::presentation::session_picker::picker_model::SessionsPickerRuntimeView;

pub(super) fn root_label(root: SessionsPickerRoot) -> &'static str {
    match root {
        SessionsPickerRoot::Cwd => "📂 cwd",
        SessionsPickerRoot::Repo => "repo",
        SessionsPickerRoot::Any => "all",
    }
}

pub(super) fn runtime_view_label(view: SessionsPickerRuntimeView) -> &'static str {
    match view {
        SessionsPickerRuntimeView::Blocked => "Blocked",
        SessionsPickerRuntimeView::Active => "Active",
        SessionsPickerRuntimeView::Idle => "Idle",
        SessionsPickerRuntimeView::All => "All",
    }
}

pub(super) fn sort_label(sort: SessionsSort) -> &'static str {
    match sort {
        SessionsSort::Updated => "updated",
        SessionsSort::Created => "created",
    }
}

pub(super) fn fit_line(line: &str, width: usize) -> String {
    truncate_middle(&line.replace('\n', " "), width)
}

pub(super) fn compact_age(value: &str) -> String {
    value
        .strip_suffix(" ago")
        .or_else(|| value.strip_prefix("in "))
        .unwrap_or(value)
        .to_owned()
}

pub(super) fn truncate_end(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 1 {
        return ".".to_owned();
    }
    let keep = max_chars - 1;
    format!("{}.", value.chars().take(keep).collect::<String>())
}

pub(super) fn truncate_middle(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 1 {
        return ".".to_owned();
    }
    let keep = max_chars - 1;
    let prefix_count = keep / 2;
    let suffix_count = keep - prefix_count;
    let prefix = value.chars().take(prefix_count).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(suffix_count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}.{suffix}")
}
