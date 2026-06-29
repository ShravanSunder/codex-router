use std::path::Path;

use crate::presentation::session_picker::request::SessionsPickerRequest;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsRoot;
use crate::sessions::SessionsSource;

pub(super) fn provider_choices(request: &SessionsPickerRequest) -> Vec<SessionsProvider> {
    let mut choices = vec![SessionsProvider::Any];
    if request.current_provider.is_some() {
        choices.push(SessionsProvider::Current);
    }
    let mut provider_ids = request
        .records
        .iter()
        .filter_map(|record| record.provider.clone())
        .collect::<Vec<_>>();
    provider_ids.sort();
    provider_ids.dedup();
    choices.extend(provider_ids.into_iter().map(SessionsProvider::Id));
    if !choices.contains(&request.provider) {
        choices.push(request.provider.clone());
    }
    choices
}

pub(super) fn root_matches(
    root: SessionsRoot,
    request: &SessionsPickerRequest,
    record: &SessionPickerRecord,
) -> bool {
    let Some(cwd) = record.cwd.as_deref().map(Path::new) else {
        return matches!(root, SessionsRoot::Any);
    };
    match root {
        SessionsRoot::Cwd => cwd == request.current_dir,
        SessionsRoot::Checkout => cwd.starts_with(&request.current_dir),
        SessionsRoot::Repo => request
            .current_dir
            .parent()
            .is_some_and(|repo_root| cwd.starts_with(repo_root)),
        SessionsRoot::Any => true,
    }
}

pub(super) fn provider_matches(
    provider: &SessionsProvider,
    request: &SessionsPickerRequest,
    record: &SessionPickerRecord,
) -> bool {
    match provider {
        SessionsProvider::Any => true,
        SessionsProvider::Current => record.provider == request.current_provider,
        SessionsProvider::Id(provider_id) => record.provider.as_ref() == Some(provider_id),
    }
}

pub(super) fn source_matches(source: SessionsSource, record: &SessionPickerRecord) -> bool {
    match source {
        SessionsSource::All => true,
        SessionsSource::Interactive => {
            matches!(record.source.as_deref(), Some("cli" | "vscode"))
                && !matches!(
                    record.thread_source.as_deref(),
                    Some("exec" | "app_server" | "subagent")
                )
        }
        SessionsSource::Subagents => {
            matches!(record.source.as_deref(), Some("subagent"))
                || matches!(record.thread_source.as_deref(), Some("subagent"))
        }
    }
}

pub(super) fn next_root_filter(root: SessionsRoot) -> SessionsRoot {
    match root {
        SessionsRoot::Cwd => SessionsRoot::Checkout,
        SessionsRoot::Checkout => SessionsRoot::Repo,
        SessionsRoot::Repo => SessionsRoot::Any,
        SessionsRoot::Any => SessionsRoot::Cwd,
    }
}

pub(super) fn next_source_filter(source: SessionsSource) -> SessionsSource {
    match source {
        SessionsSource::Interactive => SessionsSource::All,
        SessionsSource::All => SessionsSource::Subagents,
        SessionsSource::Subagents => SessionsSource::Interactive,
    }
}
