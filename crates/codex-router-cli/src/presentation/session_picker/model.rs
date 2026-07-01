use crate::presentation::session_picker::action::SessionsPickerKey;
use crate::presentation::session_picker::action::SessionsPickerOutcome;
use crate::presentation::session_picker::filters::next_root_filter;
use crate::presentation::session_picker::filters::next_sort_filter;
use crate::presentation::session_picker::filters::next_source_filter;
use crate::presentation::session_picker::filters::provider_matches;
use crate::presentation::session_picker::filters::root_matches;
use crate::presentation::session_picker::filters::source_matches;
#[cfg(test)]
use crate::presentation::session_picker::render::render_model_snapshot;
use crate::presentation::session_picker::request::SessionsPickerRequest;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsRoot;
use crate::sessions::SessionsSort;
use crate::sessions::SessionsSource;

pub(super) const VISIBLE_SESSION_ROWS: usize = 8;

/// Pure sessions picker state. iocraft owns rendering/input, this owns behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionsPickerModel {
    pub(super) request: SessionsPickerRequest,
    pub(super) width: usize,
    pub(super) root: SessionsRoot,
    pub(super) provider: SessionsProvider,
    pub(super) source: SessionsSource,
    pub(super) sort: SessionsSort,
    pub(super) search: String,
    pub(super) selected_index: usize,
}

impl SessionsPickerModel {
    pub(crate) fn new(request: SessionsPickerRequest, width: usize) -> Self {
        Self {
            root: request.root,
            provider: request.provider.clone(),
            source: request.source,
            sort: request.sort,
            request,
            width,
            search: String::new(),
            selected_index: 0,
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
            SessionsPickerKey::PageDown => {
                let visible_len = self.visible_records().len();
                if visible_len > 0 {
                    self.selected_index =
                        (self.selected_index + VISIBLE_SESSION_ROWS).min(visible_len - 1);
                }
            }
            SessionsPickerKey::PageUp => {
                self.selected_index = self.selected_index.saturating_sub(VISIBLE_SESSION_ROWS);
            }
            SessionsPickerKey::MoveFirst => {
                self.selected_index = 0;
            }
            SessionsPickerKey::MoveLast => {
                let visible_len = self.visible_records().len();
                if visible_len > 0 {
                    self.selected_index = visible_len - 1;
                }
            }
            SessionsPickerKey::CycleRoot => {
                self.root = next_root_filter(self.root);
                self.clamp_selection();
            }
            SessionsPickerKey::CycleSource => {
                self.source = next_source_filter(self.source);
                self.clamp_selection();
            }
            SessionsPickerKey::CycleSort => {
                self.sort = next_sort_filter(self.sort);
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
            SessionsPickerKey::ClearSearch => {
                self.search.clear();
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

    #[cfg(test)]
    pub(crate) fn render_snapshot(&self) -> String {
        render_model_snapshot(self)
    }

    pub(super) fn visible_records(&self) -> Vec<&SessionPickerRecord> {
        let search = self.search.to_lowercase();
        let mut records = self
            .request
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
            .collect::<Vec<_>>();
        records.sort_by(|left, right| match self.sort {
            SessionsSort::Updated => right
                .recency_at_ms
                .unwrap_or(i64::MIN)
                .cmp(&left.recency_at_ms.unwrap_or(i64::MIN)),
            SessionsSort::Created => right
                .created_at_ms
                .unwrap_or(i64::MIN)
                .cmp(&left.created_at_ms.unwrap_or(i64::MIN)),
        });
        records
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

pub(super) fn visible_window_start(
    selected_index: usize,
    visible_len: usize,
    max_visible: usize,
) -> usize {
    if visible_len <= max_visible {
        return 0;
    }
    selected_index
        .saturating_add(1)
        .saturating_sub(max_visible)
        .min(visible_len.saturating_sub(max_visible))
}
