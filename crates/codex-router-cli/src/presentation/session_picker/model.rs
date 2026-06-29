use crate::presentation::session_picker::action::SessionsPickerKey;
use crate::presentation::session_picker::action::SessionsPickerOutcome;
use crate::presentation::session_picker::filters::next_root_filter;
use crate::presentation::session_picker::filters::next_source_filter;
use crate::presentation::session_picker::filters::provider_choices;
use crate::presentation::session_picker::filters::provider_matches;
use crate::presentation::session_picker::filters::root_matches;
use crate::presentation::session_picker::filters::source_matches;
use crate::presentation::session_picker::render::render_model_snapshot;
use crate::presentation::session_picker::request::SessionsPickerRequest;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsRoot;
use crate::sessions::SessionsSource;

/// Pure sessions picker state. iocraft owns rendering/input, this owns behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionsPickerModel {
    pub(super) request: SessionsPickerRequest,
    pub(super) width: usize,
    pub(super) root: SessionsRoot,
    pub(super) provider: SessionsProvider,
    pub(super) source: SessionsSource,
    pub(super) search: String,
    pub(super) selected_index: usize,
    provider_choices: Vec<SessionsProvider>,
}

impl SessionsPickerModel {
    pub(crate) fn new(request: SessionsPickerRequest, width: usize) -> Self {
        let provider_choices = provider_choices(&request);
        Self {
            root: request.root,
            provider: request.provider.clone(),
            source: request.source,
            request,
            width,
            search: String::new(),
            selected_index: 0,
            provider_choices,
        }
    }

    pub(crate) fn handle_key(&mut self, key: SessionsPickerKey) {
        match key {
            SessionsPickerKey::MoveDown => {
                let visible_len = self.visible_records().len();
                if visible_len > 0 {
                    self.selected_index = (self.selected_index + 1).min(visible_len - 1);
                }
            }
            SessionsPickerKey::MoveUp => {
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            SessionsPickerKey::CycleRoot => {
                self.root = next_root_filter(self.root);
                self.clamp_selection();
            }
            SessionsPickerKey::CycleProvider => {
                let current_index = self
                    .provider_choices
                    .iter()
                    .position(|provider| provider == &self.provider)
                    .unwrap_or(0);
                let next_index = (current_index + 1) % self.provider_choices.len().max(1);
                if let Some(provider) = self.provider_choices.get(next_index) {
                    self.provider = provider.clone();
                }
                self.clamp_selection();
            }
            SessionsPickerKey::CycleSource => {
                self.source = next_source_filter(self.source);
                self.clamp_selection();
            }
            SessionsPickerKey::SearchChar(character) => {
                if !character.is_control() {
                    self.search.push(character);
                }
                self.clamp_selection();
            }
            SessionsPickerKey::SearchBackspace => {
                self.search.pop();
                self.clamp_selection();
            }
        }
    }

    pub(crate) fn selected_session_id(&self) -> Option<&str> {
        self.visible_records()
            .get(self.selected_index)
            .map(|record| record.session_id.as_str())
    }

    pub(crate) fn selected_outcome(&self) -> Option<SessionsPickerOutcome> {
        match self.selected_session_id() {
            Some(session_id) => Some(SessionsPickerOutcome::ResumeSession(session_id.to_owned())),
            None => Some(SessionsPickerOutcome::StartNewSession),
        }
    }

    pub(crate) fn render_snapshot(&self) -> String {
        render_model_snapshot(self)
    }

    pub(super) fn visible_records(&self) -> Vec<&SessionPickerRecord> {
        let search = self.search.to_lowercase();
        self.request
            .records
            .iter()
            .filter(|record| root_matches(self.root, &self.request, record))
            .filter(|record| provider_matches(&self.provider, &self.request, record))
            .filter(|record| source_matches(self.source, record))
            .filter(|record| {
                search.is_empty()
                    || record.title.to_lowercase().contains(&search)
                    || record.session_id.to_lowercase().contains(&search)
                    || record
                        .provider
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&search)
            })
            .collect()
    }

    fn clamp_selection(&mut self) {
        let visible_len = self.visible_records().len();
        if visible_len == 0 {
            self.selected_index = 0;
        } else if self.selected_index >= visible_len {
            self.selected_index = visible_len - 1;
        }
    }
}
