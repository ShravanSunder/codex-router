//! Picker display text.
use super::{SessionsPickerRoot, SessionsSort};
use crate::presentation::session_picker::picker_model::SessionsPickerRuntimeView;
use unicode_width::UnicodeWidthStr;

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

pub(in crate::session_picker) fn fit_line(line: &str, width: usize) -> String {
    let line = line.replace('\n', " ");
    if UnicodeWidthStr::width(line.as_str()) <= width {
        return line;
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return ".".to_owned();
    }
    let prefix_budget = (width - 1) / 2;
    let suffix_budget = width - 1 - prefix_budget;
    let mut prefix = String::new();
    for character in line.chars() {
        prefix.push(character);
        if UnicodeWidthStr::width(prefix.as_str()) > prefix_budget {
            prefix.pop();
            break;
        }
    }
    let mut suffix = String::new();
    for character in line.chars().rev() {
        let candidate = format!("{character}{suffix}");
        if UnicodeWidthStr::width(candidate.as_str()) > suffix_budget {
            break;
        }
        suffix = candidate;
    }
    format!("{prefix}.{suffix}")
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
