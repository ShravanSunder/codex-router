use std::path::PathBuf;

use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsRoot;
use crate::sessions::SessionsSource;

/// Request needed to render and drive the interactive sessions picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionsPickerRequest {
    pub(crate) root: SessionsRoot,
    pub(crate) provider: SessionsProvider,
    pub(crate) source: SessionsSource,
    pub(crate) current_dir: PathBuf,
    pub(crate) checkout_root: PathBuf,
    pub(crate) repo_roots: Vec<PathBuf>,
    pub(crate) current_provider: Option<String>,
    pub(crate) records: Vec<SessionPickerRecord>,
}

impl Default for SessionsPickerRequest {
    fn default() -> Self {
        Self {
            root: SessionsRoot::Cwd,
            provider: SessionsProvider::Any,
            source: SessionsSource::Interactive,
            current_dir: PathBuf::new(),
            checkout_root: PathBuf::new(),
            repo_roots: Vec::new(),
            current_provider: None,
            records: Vec::new(),
        }
    }
}
