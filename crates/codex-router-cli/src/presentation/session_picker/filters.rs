use std::path::Path;

use crate::presentation::session_picker::request::SessionsPickerRequest;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsRoot;
use crate::sessions::SessionsSource;

pub(super) fn root_matches(
    root: SessionsRoot,
    request: &SessionsPickerRequest,
    record: &SessionPickerRecord,
) -> bool {
    let Some(cwd) = record.cwd.as_deref().map(Path::new) else {
        return matches!(root, SessionsRoot::Any);
    };
    let cwd = normalize_path(cwd);
    match root {
        SessionsRoot::Cwd => cwd == normalize_path(&request.current_dir),
        SessionsRoot::Checkout => path_is_equal_or_child(&cwd, &request.checkout_root),
        SessionsRoot::Repo => request
            .repo_roots
            .iter()
            .any(|repo_root| path_is_equal_or_child(&cwd, repo_root)),
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

fn normalize_path(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_error| path.to_path_buf())
}

fn path_is_equal_or_child(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}
