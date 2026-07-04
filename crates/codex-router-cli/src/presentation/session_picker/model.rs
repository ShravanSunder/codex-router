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
use crate::presentation::session_picker::request::SessionsPickerDataQuery;
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
    visible_indices: Vec<usize>,
    visible_rows_generation: usize,
}

impl SessionsPickerModel {
    pub(crate) fn new(request: SessionsPickerRequest, width: usize) -> Self {
        let mut model = Self {
            root: request.root,
            provider: request.provider.clone(),
            source: request.source,
            sort: request.sort,
            request,
            width,
            search: String::new(),
            selected_index: 0,
            visible_indices: Vec::new(),
            visible_rows_generation: 0,
        };
        model.rebuild_visible_rows();
        model.select_first_record_when_available();
        model
    }

    pub(crate) fn handle_key(&mut self, key: SessionsPickerKey) {
        match key {
            SessionsPickerKey::MoveDown => {
                let visible_len = self.visible_len();
                if visible_len > 0 {
                    self.selected_index = (self.selected_index + 1).min(visible_len - 1);
                }
            }
            SessionsPickerKey::MoveUp => {
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            SessionsPickerKey::PageDown => {
                let visible_len = self.visible_len();
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
                let visible_len = self.visible_len();
                if visible_len > 0 {
                    self.selected_index = visible_len - 1;
                }
            }
            SessionsPickerKey::CycleRoot => {
                self.root = next_root_filter(self.root);
                self.rebuild_visible_rows();
                self.clamp_selection();
            }
            SessionsPickerKey::CycleSource => {
                self.source = next_source_filter(self.source);
                self.rebuild_visible_rows();
                self.clamp_selection();
            }
            SessionsPickerKey::CycleSort => {
                self.sort = next_sort_filter(self.sort);
                self.rebuild_visible_rows();
                self.clamp_selection();
            }
            SessionsPickerKey::SearchChar(character) => {
                if !character.is_control() {
                    self.search.push(character);
                    self.rebuild_visible_rows();
                }
                self.clamp_selection();
            }
            SessionsPickerKey::SearchBackspace => {
                if self.search.pop().is_some() {
                    self.rebuild_visible_rows();
                    self.clamp_selection();
                }
            }
            SessionsPickerKey::ClearSearch => {
                if !self.search.is_empty() {
                    self.search.clear();
                    self.rebuild_visible_rows();
                    self.clamp_selection();
                }
            }
        }
    }

    pub(crate) fn set_width(&mut self, width: usize) {
        self.width = width;
    }

    pub(crate) fn data_query(&self) -> SessionsPickerDataQuery {
        SessionsPickerDataQuery {
            root: self.root,
            provider: self.provider.clone(),
            source: self.source,
            sort: self.sort,
            search: self.search.clone(),
        }
    }

    pub(crate) fn replace_records(&mut self, records: Vec<SessionPickerRecord>) {
        self.request.records = records;
        self.rebuild_visible_rows();
        self.clamp_selection();
    }

    pub(crate) fn selected_session_id(&self) -> Option<&str> {
        self.selected_record()
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

    #[cfg(test)]
    pub(crate) fn visible_rows_generation(&self) -> usize {
        self.visible_rows_generation
    }

    pub(super) fn visible_len(&self) -> usize {
        self.visible_indices.len() + 1
    }

    pub(super) fn visible_record_len(&self) -> usize {
        self.visible_indices.len()
    }

    pub(super) fn selected_record(&self) -> Option<&SessionPickerRecord> {
        self.visible_choice_record_at(self.selected_index)
    }

    pub(super) fn visible_choice_record_at(&self, index: usize) -> Option<&SessionPickerRecord> {
        if index == 0 {
            return None;
        }
        self.visible_record_at(index - 1)
    }

    pub(super) fn visible_record_at(&self, index: usize) -> Option<&SessionPickerRecord> {
        self.visible_indices
            .get(index)
            .and_then(|record_index| self.request.records.get(*record_index))
    }

    fn rebuild_visible_rows(&mut self) {
        let search = self.search.to_lowercase();
        let mut indices = self
            .request
            .records
            .iter()
            .enumerate()
            .filter(|(_index, record)| root_matches(self.root, &self.request, record))
            .filter(|(_index, record)| provider_matches(&self.provider, &self.request, record))
            .filter(|(_index, record)| source_matches(self.source, record))
            .filter(|(_index, record)| {
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
            .map(|(index, _record)| index)
            .collect::<Vec<_>>();
        indices.sort_by(|left_index, right_index| {
            let Some(left) = self.request.records.get(*left_index) else {
                return left_index.cmp(right_index);
            };
            let Some(right) = self.request.records.get(*right_index) else {
                return left_index.cmp(right_index);
            };
            match self.sort {
                SessionsSort::Updated => right
                    .recency_at_ms
                    .unwrap_or(i64::MIN)
                    .cmp(&left.recency_at_ms.unwrap_or(i64::MIN)),
                SessionsSort::Created => right
                    .created_at_ms
                    .unwrap_or(i64::MIN)
                    .cmp(&left.created_at_ms.unwrap_or(i64::MIN)),
            }
        });
        self.visible_indices = indices;
        self.visible_rows_generation = self.visible_rows_generation.saturating_add(1);
    }

    fn select_first_record_when_available(&mut self) {
        if !self.visible_indices.is_empty() {
            self.selected_index = 1;
        }
    }

    fn clamp_selection(&mut self) {
        let visible_len = self.visible_len();
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
