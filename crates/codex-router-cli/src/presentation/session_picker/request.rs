use std::path::PathBuf;
use std::sync::Arc;

use crate::sessions::RepositoryIdentity;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsSort;
use crate::sessions::SessionsSource;

/// Request needed to render and drive the interactive sessions picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionsPickerRequest {
    pub(crate) root: SessionsPickerRoot,
    pub(crate) provider: SessionsProvider,
    pub(crate) source: SessionsSource,
    pub(crate) sort: SessionsSort,
    pub(crate) current_dir: PathBuf,
    pub(crate) repository_identity: RepositoryIdentity,
    pub(crate) current_provider: Option<String>,
    pub(crate) new_session_args_display: String,
    pub(crate) records: Vec<SessionPickerRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionsPickerDataQuery {
    pub(crate) root: SessionsPickerRoot,
    pub(crate) provider: SessionsProvider,
    pub(crate) source: SessionsSource,
    pub(crate) sort: SessionsSort,
    pub(crate) search: String,
}

pub(crate) type SessionsPickerRecordLoader =
    Arc<dyn Fn(SessionsPickerDataQuery) -> Result<Vec<SessionPickerRecord>, String> + Send + Sync>;

impl Default for SessionsPickerRequest {
    fn default() -> Self {
        Self {
            root: SessionsPickerRoot::Cwd,
            provider: SessionsProvider::Any,
            source: SessionsSource::Interactive,
            sort: SessionsSort::Updated,
            current_dir: PathBuf::new(),
            repository_identity: RepositoryIdentity {
                normalized_origin: None,
                live_roots: Vec::new(),
                repository_basename: String::new(),
                fallback_cwd: None,
            },
            current_provider: None,
            new_session_args_display: String::new(),
            records: Vec::new(),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionsPickerRoot {
    Cwd,
    Repo,
    Any,
}
