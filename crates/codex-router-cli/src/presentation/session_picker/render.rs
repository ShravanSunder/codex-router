use crate::presentation::session_picker::model::SessionsPickerModel;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsRoot;
use crate::sessions::SessionsSource;

pub(super) const MIN_PICKER_WIDTH: usize = 24;
const NARROW_PICKER_WIDTH: usize = 72;
const ULTRA_NARROW_PICKER_WIDTH: usize = 40;

pub(super) fn render_model_snapshot(model: &SessionsPickerModel) -> String {
    if model.width < MIN_PICKER_WIDTH {
        return "terminal too narrow\n".to_owned();
    }

    let visible_records = model.visible_records();
    let mut lines = vec![
        fit_line("Resume Codex session", model.width),
        render_filters_line(model),
        fit_line(&format!("Search {}", model.search), model.width),
    ];

    if visible_records.is_empty() {
        lines.push(fit_line("> Start new session", model.width));
        lines.push(fit_line(
            "No existing sessions match; enter starts a new router-profile session",
            model.width,
        ));
    } else {
        for (index, record) in visible_records.iter().take(8).enumerate() {
            let marker = if index == model.selected_index {
                ">"
            } else {
                " "
            };
            lines.push(fit_line(&format!("{marker} {}", record.title), model.width));
            lines.push(fit_line(
                &format!(
                    "  {}  {}  {}  {}  id={}",
                    record.recency,
                    record.branch,
                    record.context,
                    record.provider.as_deref().unwrap_or("-"),
                    short_id(&record.session_id)
                ),
                model.width,
            ));
        }
        if model.width >= NARROW_PICKER_WIDTH
            && let Some(record) = visible_records.get(model.selected_index)
        {
            lines.push(fit_line("Details", model.width));
            lines.push(fit_line(&format!("title {}", record.title), model.width));
            lines.push(fit_line(
                &format!("cwd {}", record.cwd.as_deref().unwrap_or("-")),
                model.width,
            ));
            lines.push(fit_line(&format!("branch {}", record.branch), model.width));
            lines.push(fit_line(
                &format!("provider {}", record.provider.as_deref().unwrap_or("-")),
                model.width,
            ));
            lines.push(fit_line(
                &format!("model {}", record.model.as_deref().unwrap_or("-")),
                model.width,
            ));
            lines.push(fit_line(
                &format!("id {}", short_id(&record.session_id)),
                model.width,
            ));
            lines.push(fit_line(
                &format!("preview {}", record.preview.as_deref().unwrap_or("-")),
                model.width,
            ));
        }
    }

    lines.push(fit_line(
        "Keys: up/down select  enter resume/new  tab root  shift-tab provider  ctrl-s source  esc cancel",
        model.width,
    ));
    format!("{}\n", lines.join("\n"))
}

fn render_filters_line(model: &SessionsPickerModel) -> String {
    let root = format!("Root {}", root_label(model.root));
    let provider = format!("Provider {}", provider_label(&model.provider));
    let source = format!("Source {}", source_label(model.source));
    if model.width < ULTRA_NARROW_PICKER_WIDTH {
        return [root, provider, source]
            .into_iter()
            .map(|line| fit_line(&line, model.width))
            .collect::<Vec<_>>()
            .join("\n");
    }
    fit_line(&format!("{root}  {provider}  {source}"), model.width)
}

fn root_label(root: SessionsRoot) -> &'static str {
    match root {
        SessionsRoot::Cwd => "cwd",
        SessionsRoot::Checkout => "checkout",
        SessionsRoot::Repo => "repo",
        SessionsRoot::Any => "any",
    }
}

fn provider_label(provider: &SessionsProvider) -> String {
    match provider {
        SessionsProvider::Any => "any".to_owned(),
        SessionsProvider::Current => "current".to_owned(),
        SessionsProvider::Id(provider_id) => provider_id.clone(),
    }
}

fn source_label(source: SessionsSource) -> &'static str {
    match source {
        SessionsSource::Interactive => "interactive",
        SessionsSource::All => "all",
        SessionsSource::Subagents => "subagents",
    }
}

fn short_id(session_id: &str) -> String {
    truncate_middle(session_id, 12)
}

fn fit_line(line: &str, width: usize) -> String {
    let line = line.replace('\n', " ");
    truncate_middle(&line, width)
}

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
    use crate::presentation::session_picker::model::SessionsPickerModel;
    use crate::presentation::session_picker::test_support::picker_request;

    #[test]
    fn sessions_picker_width_snapshots_fit_without_table_sprawl() {
        let wide = SessionsPickerModel::new(picker_request(), 100).render_snapshot();
        assert!(wide.contains("Details"));
        assert!(wide.contains("title Feature design session"));
        assert!(wide.contains("cwd /repo/project-a"));
        assert!(wide.contains("branch main"));
        assert!(wide.contains("provider codex-router"));
        assert!(wide.contains("model gpt-5-codex"));
        assert!(wide.contains("id thread-a"));
        assert!(wide.contains("preview Feature design session preview text"));
        assert!(wide.lines().all(|line| line.chars().count() <= 100));

        let narrow = SessionsPickerModel::new(picker_request(), 64).render_snapshot();
        assert!(narrow.contains("Root cwd  Provider any  Source interactive"));
        assert!(narrow.lines().all(|line| line.chars().count() <= 64));

        let ultra_narrow = SessionsPickerModel::new(picker_request(), 36).render_snapshot();
        assert!(ultra_narrow.contains("Root cwd"));
        assert!(ultra_narrow.contains("Provider any"));
        assert!(ultra_narrow.contains("Source interactive"));
        assert!(ultra_narrow.contains('…'));
        assert!(ultra_narrow.lines().all(|line| line.chars().count() <= 36));

        let too_narrow = SessionsPickerModel::new(picker_request(), 20).render_snapshot();
        assert_eq!(too_narrow.trim(), "terminal too narrow");
    }
}
