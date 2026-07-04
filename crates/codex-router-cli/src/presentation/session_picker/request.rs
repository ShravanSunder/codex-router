use std::path::PathBuf;
use std::sync::Arc;

use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsRoot;
use crate::sessions::SessionsSort;
use crate::sessions::SessionsSource;

/// Request needed to render and drive the interactive sessions picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionsPickerRequest {
    pub(crate) root: SessionsRoot,
    pub(crate) provider: SessionsProvider,
    pub(crate) source: SessionsSource,
    pub(crate) sort: SessionsSort,
    pub(crate) current_dir: PathBuf,
    pub(crate) checkout_root: PathBuf,
    pub(crate) repo_roots: Vec<PathBuf>,
    pub(crate) current_provider: Option<String>,
    pub(crate) records: Vec<SessionPickerRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionsPickerDataQuery {
    pub(crate) root: SessionsRoot,
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
            root: SessionsRoot::Cwd,
            provider: SessionsProvider::Any,
            source: SessionsSource::Interactive,
            sort: SessionsSort::Updated,
            current_dir: PathBuf::new(),
            checkout_root: PathBuf::new(),
            repo_roots: Vec::new(),
            current_provider: None,
            records: Vec::new(),
        }
    }
}
