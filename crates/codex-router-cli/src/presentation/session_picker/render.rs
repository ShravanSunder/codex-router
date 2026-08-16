use unicode_width::UnicodeWidthStr;

#[cfg(test)]
use crate::presentation::session_picker::model::SessionsPickerModel;
#[cfg(test)]
use crate::presentation::session_picker::model::VISIBLE_SESSION_ROWS;
#[cfg(test)]
use crate::presentation::session_picker::model::visible_window_start;
#[cfg(test)]
use crate::presentation::session_picker::request::SessionsPickerRoot;
#[cfg(test)]
use crate::sessions::SessionsSort;
#[cfg(test)]
use crate::sessions::SessionsSource;

pub(super) const MIN_PICKER_WIDTH: usize = 24;
pub(super) const SEARCH_HELP: &str = "Search: id:<id> | b:<branch> | repo:<name>";
pub(super) const SHORTCUT_HELP: &str = "ctrl-n new | ctrl-s scope | ctrl-t threads | ctrl-o sort";
const FULL_HELP: &str = "Search: id:<id> | b:<branch> | repo:<name>    ctrl-n new | ctrl-s scope | ctrl-t threads | ctrl-o sort";
#[cfg(test)]
const NARROW_PICKER_WIDTH: usize = 72;
#[cfg(test)]
const ULTRA_NARROW_PICKER_WIDTH: usize = 40;

#[cfg(test)]
pub(super) fn render_model_snapshot(model: &SessionsPickerModel) -> String {
    if model.width < MIN_PICKER_WIDTH {
        return "terminal too narrow\n".to_owned();
    }

    let visible_len = model.visible_len();
    let mut lines = vec![fit_line("Resume a previous session", model.width)];
    lines.extend(render_controls_lines(model));

    if visible_len > 0 {
        let window_start = visible_window_start(
            model.focused_visible_index(),
            visible_len,
            VISIBLE_SESSION_ROWS,
        );
        if window_start > 0 {
            lines.push(fit_line(
                &format!("+{window_start} more above"),
                model.width,
            ));
        }
        let window_end = (window_start + VISIBLE_SESSION_ROWS).min(visible_len);
        for visible_index in window_start..window_end {
            let marker = if visible_index == model.focused_visible_index() {
                "❯"
            } else {
                " "
            };
            if visible_index == 0 {
                lines.push(fit_line(
                    &format!("{marker} Start new session"),
                    model.width,
                ));
                lines.push(fit_line(
                    &format!("  {}", start_new_args_label(model)),
                    model.width,
                ));
                if model.visible_record_len() == 0 {
                    lines.push(fit_line(
                        if model.search.is_empty() {
                            "No existing sessions match these filters"
                        } else {
                            "No matching sessions"
                        },
                        model.width,
                    ));
                }
                continue;
            }
            let Some(record) = model.visible_choice_record_at(visible_index) else {
                continue;
            };
            let title_width = model.width.saturating_sub(18).max(14);
            lines.push(fit_line(
                &format!(
                    "{marker} {:<title_width$} {:>6} {:>6}",
                    truncate_end(&record.title, title_width),
                    compact_age(&record.recency),
                    compact_age(&record.created)
                ),
                model.width,
            ));
            lines.push(fit_line(
                &format!(
                    "  ⎇ {}  📂 {}",
                    record.branch,
                    record.cwd.as_deref().unwrap_or("-")
                ),
                model.width,
            ));
        }
        let remaining = visible_len.saturating_sub(window_start + VISIBLE_SESSION_ROWS);
        if remaining > 0 {
            lines.push(fit_line(&format!("+{remaining} more below"), model.width));
        }
        if model.width >= NARROW_PICKER_WIDTH
            && let Some(record) = model.focused_record()
        {
            lines.push(fit_line(
                &format!("Session ID: {}", record.session_id),
                model.width,
            ));
            lines.push(String::new());
            lines.push(fit_line("Conversation", model.width));
            if record.conversation.snippets.is_empty() {
                lines.push(fit_line(
                    record
                        .conversation
                        .unavailable_reason
                        .as_deref()
                        .unwrap_or("history unavailable"),
                    model.width,
                ));
            } else {
                for snippet in &record.conversation.snippets {
                    lines.push(fit_line(&format!("• {snippet}"), model.width));
                }
            }
        }
    }

    lines.extend(
        footer_lines(model.width)
            .into_iter()
            .map(|line| fit_line(&line, model.width)),
    );
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
fn start_new_args_label(model: &SessionsPickerModel) -> String {
    if model.request.new_session_args_display.is_empty() {
        "no extra args".to_owned()
    } else {
        format!("args: {}", model.request.new_session_args_display)
    }
}

#[cfg(test)]
fn render_controls_lines(model: &SessionsPickerModel) -> Vec<String> {
    let search = format!("Search: [{}]", model.search);
    let root = format!("[{}]", root_label(model.root));
    let source = format!("Threads: [{}]", source_label(model.source));
    let sort = format!("Sort: [{}]", sort_label(model.sort));
    let combined = format!("{search}    {root}    {source}    {sort}");
    if UnicodeWidthStr::width(combined.as_str()) <= model.width {
        return vec![fit_line(&combined, model.width)];
    }
    if model.width < ULTRA_NARROW_PICKER_WIDTH {
        return [search, root, source, sort]
            .into_iter()
            .map(|line| fit_line(&line, model.width))
            .collect();
    }
    vec![
        fit_line(&search, model.width),
        fit_line(&format!("{root}    {source}    {sort}"), model.width),
    ]
}

pub(super) fn footer_lines(width: usize) -> Vec<String> {
    if UnicodeWidthStr::width(FULL_HELP) <= width {
        return vec![FULL_HELP.to_owned()];
    }

    let mut lines = if UnicodeWidthStr::width(SEARCH_HELP) <= width {
        vec![SEARCH_HELP.to_owned()]
    } else {
        vec![
            "Search: id:<id>".to_owned(),
            "b:<branch> | repo:<name>".to_owned(),
        ]
    };

    if UnicodeWidthStr::width(SHORTCUT_HELP) <= width {
        lines.push(SHORTCUT_HELP.to_owned());
    } else if width >= 28 {
        lines.extend([
            "ctrl-n new | ctrl-s scope".to_owned(),
            "ctrl-t threads | ctrl-o sort".to_owned(),
        ]);
    } else {
        lines.extend([
            "ctrl-n new".to_owned(),
            "| ctrl-s scope".to_owned(),
            "| ctrl-t threads".to_owned(),
            "| ctrl-o sort".to_owned(),
        ]);
    }
    lines
}

#[cfg(test)]
fn sort_label(sort: SessionsSort) -> &'static str {
    match sort {
        SessionsSort::Updated => "updated",
        SessionsSort::Created => "created",
    }
}

#[cfg(test)]
fn root_label(root: SessionsPickerRoot) -> &'static str {
    match root {
        SessionsPickerRoot::Cwd => "📂 cwd",
        SessionsPickerRoot::Repo => "repo",
        SessionsPickerRoot::Any => "all",
    }
}

#[cfg(test)]
fn source_label(source: SessionsSource) -> &'static str {
    match source {
        SessionsSource::Interactive => "interactive",
        SessionsSource::All => "all",
        SessionsSource::Subagents => "subagents",
    }
}

#[cfg(test)]
fn fit_line(line: &str, width: usize) -> String {
    let line = line.replace('\n', " ");
    truncate_middle(&line, width)
}

#[cfg(test)]
fn compact_age(value: &str) -> String {
    value
        .strip_suffix(" ago")
        .or_else(|| value.strip_prefix("in "))
        .unwrap_or(value)
        .to_owned()
}

#[cfg(test)]
fn truncate_end(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 1 {
        return "…".to_owned();
    }
    let keep = max_chars - 1;
    format!("{}…", value.chars().take(keep).collect::<String>())
}

#[cfg(test)]
fn truncate_middle(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 1 {
        return "…".to_owned();
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
    format!("{prefix}…{suffix}")
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::footer_lines;
    use crate::presentation::session_picker::model::SessionsPickerModel;
    use crate::presentation::session_picker::test_support::picker_request;

    #[test]
    fn sessions_picker_width_snapshots_fit_without_table_sprawl() {
        let wide = SessionsPickerModel::new(picker_request(), 100).render_snapshot();
        assert!(wide.contains("Search: []"));
        assert!(!wide.contains("Search text, id:, b:branch, repo:name"));
        assert!(wide.contains("Session ID: thread-a\n\nConversation"));
        assert!(wide.contains("Conversation"));
        assert!(wide.contains("Feature design session first real message"));
        assert!(!wide.contains("Preview"));
        assert!(!wide.contains("Metadata"));
        assert!(wide.lines().all(|line| line.chars().count() <= 100));

        let narrow = SessionsPickerModel::new(picker_request(), 64).render_snapshot();
        assert!(narrow.contains("Search: []"));
        assert!(narrow.contains("[📂 cwd]"));
        assert!(narrow.contains("Threads: [interactive]"));
        assert!(narrow.lines().all(|line| line.chars().count() <= 64));

        let compact = SessionsPickerModel::new(picker_request(), 52).render_snapshot();
        assert!(compact.contains("[📂 cwd]"));
        assert!(compact.contains("Threads: [interactive]"));
        assert!(compact.contains("Sort: [updated]"));
        assert!(compact.lines().all(|line| line.chars().count() <= 52));

        let ultra_narrow = SessionsPickerModel::new(picker_request(), 36).render_snapshot();
        assert!(ultra_narrow.contains("[📂 cwd]"));
        assert!(ultra_narrow.contains("Threads: [interactive]"));
        assert!(ultra_narrow.contains("Sort: [updated]"));
        assert!(ultra_narrow.contains("Search: id:<id>"));
        assert!(ultra_narrow.contains("b:<branch> | repo:<name>"));
        assert!(ultra_narrow.contains("ctrl-n new | ctrl-s scope"));
        assert!(ultra_narrow.contains("ctrl-t threads | ctrl-o sort"));
        assert!(ultra_narrow.contains('…'));
        assert!(ultra_narrow.lines().all(|line| line.chars().count() <= 36));

        let too_narrow = SessionsPickerModel::new(picker_request(), 20).render_snapshot();
        assert_eq!(too_narrow.trim(), "terminal too narrow");
    }

    #[test]
    fn sessions_picker_footer_preserves_help_groups_at_responsive_boundaries() {
        for width in [24, 28, 42, 56, 102] {
            let lines = footer_lines(width);
            let text = lines.join("\n");

            assert!(text.contains("id:<id>"), "width {width}: {text}");
            assert!(text.contains("b:<branch>"), "width {width}: {text}");
            assert!(text.contains("repo:<name>"), "width {width}: {text}");
            assert!(text.contains("ctrl-n new"), "width {width}: {text}");
            assert!(text.contains("ctrl-s scope"), "width {width}: {text}");
            assert!(text.contains("ctrl-t threads"), "width {width}: {text}");
            assert!(text.contains("ctrl-o sort"), "width {width}: {text}");
            assert!(
                lines
                    .iter()
                    .all(|line| UnicodeWidthStr::width(line.as_str()) <= width),
                "width {width}: {text}"
            );
        }
    }
}
