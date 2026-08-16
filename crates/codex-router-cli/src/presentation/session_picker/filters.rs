use std::path::Path;

use crate::presentation::session_picker::request::SessionsPickerRequest;
use crate::presentation::session_picker::request::SessionsPickerRoot;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsSort;
use crate::sessions::SessionsSource;
use crate::sessions::normalized_paths_resolve_to_same_location;
use crate::sessions::session_belongs_to_repository;

pub(super) fn root_matches(
    root: SessionsPickerRoot,
    request: &SessionsPickerRequest,
    record: &SessionPickerRecord,
) -> bool {
    let Some(cwd) = record.normalized_cwd.as_deref().map(Path::new) else {
        return matches!(root, SessionsPickerRoot::Any);
    };
    match root {
        SessionsPickerRoot::Cwd => {
            normalized_paths_resolve_to_same_location(cwd, &request.current_dir)
        }
        SessionsPickerRoot::Repo => session_belongs_to_repository(
            &request.repository_identity,
            record.git_origin_url.as_deref(),
            cwd,
        ),
        SessionsPickerRoot::Any => true,
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

pub(super) fn next_root_filter(root: SessionsPickerRoot) -> SessionsPickerRoot {
    match root {
        SessionsPickerRoot::Cwd => SessionsPickerRoot::Repo,
        SessionsPickerRoot::Repo => SessionsPickerRoot::Any,
        SessionsPickerRoot::Any => SessionsPickerRoot::Cwd,
    }
}

pub(super) fn next_source_filter(source: SessionsSource) -> SessionsSource {
    match source {
        SessionsSource::Interactive => SessionsSource::All,
        SessionsSource::All => SessionsSource::Subagents,
        SessionsSource::Subagents => SessionsSource::Interactive,
    }
}

pub(super) fn next_sort_filter(sort: SessionsSort) -> SessionsSort {
    match sort {
        SessionsSort::Updated => SessionsSort::Created,
        SessionsSort::Created => SessionsSort::Updated,
    }
}

#[cfg(test)]
mod tests {
    use super::root_matches;
    use crate::presentation::session_picker::request::SessionsPickerRoot;
    use crate::presentation::session_picker::test_support::picker_request;

    #[test]
    fn cwd_scope_treats_var_and_private_var_as_the_same_location() {
        let mut request = picker_request();
        request.current_dir = "/private/var/folders/session-work".into();
        let mut record = request.records.remove(0);
        record.normalized_cwd = Some("/var/folders/session-work".to_owned());

        assert!(root_matches(SessionsPickerRoot::Cwd, &request, &record));
    }
}
