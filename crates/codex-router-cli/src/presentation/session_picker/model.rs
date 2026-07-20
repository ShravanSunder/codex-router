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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) enum SessionsPickerFocus {
    #[default]
    StartNew,
    SessionId(String),
}

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
    focus: SessionsPickerFocus,
    pointer_window_start: Option<usize>,
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
            focus: SessionsPickerFocus::StartNew,
            pointer_window_start: None,
            visible_indices: Vec::new(),
            visible_rows_generation: 0,
        };
        model.rebuild_visible_rows();
        model.focus_first_record_when_available();
        model
    }

    pub(crate) fn handle_key(&mut self, key: SessionsPickerKey) {
        self.pointer_window_start = None;
        match key {
            SessionsPickerKey::MoveDown => {
                let visible_len = self.visible_len();
                self.focus_visible_index(
                    (self.focused_visible_index() + 1).min(visible_len.saturating_sub(1)),
                );
            }
            SessionsPickerKey::MoveUp => {
                self.focus_visible_index(self.focused_visible_index().saturating_sub(1));
            }
            SessionsPickerKey::PageDown => {
                let visible_len = self.visible_len();
                self.focus_visible_index(
                    (self.focused_visible_index() + VISIBLE_SESSION_ROWS)
                        .min(visible_len.saturating_sub(1)),
                );
            }
            SessionsPickerKey::PageUp => {
                self.focus_visible_index(
                    self.focused_visible_index()
                        .saturating_sub(VISIBLE_SESSION_ROWS),
                );
            }
            SessionsPickerKey::MoveFirst => {
                self.focus_start_new();
            }
            SessionsPickerKey::MoveLast => {
                let visible_len = self.visible_len();
                self.focus_visible_index(visible_len.saturating_sub(1));
            }
            SessionsPickerKey::CycleRoot => {
                let previous_index = self.focused_visible_index();
                self.root = next_root_filter(self.root);
                self.rebuild_visible_rows();
                self.restore_focus_or_fallback(previous_index);
            }
            SessionsPickerKey::CycleSource => {
                let previous_index = self.focused_visible_index();
                self.source = next_source_filter(self.source);
                self.rebuild_visible_rows();
                self.restore_focus_or_fallback(previous_index);
            }
            SessionsPickerKey::CycleSort => {
                let previous_index = self.focused_visible_index();
                self.sort = next_sort_filter(self.sort);
                self.rebuild_visible_rows();
                self.restore_focus_or_fallback(previous_index);
            }
            SessionsPickerKey::SearchChar(character) => {
                let previous_index = self.focused_visible_index();
                if !character.is_control() {
                    self.search.push(character);
                    self.rebuild_visible_rows();
                }
                self.restore_focus_or_fallback(previous_index);
            }
            SessionsPickerKey::SearchBackspace => {
                let previous_index = self.focused_visible_index();
                if self.search.pop().is_some() {
                    self.rebuild_visible_rows();
                    self.restore_focus_or_fallback(previous_index);
                }
            }
            SessionsPickerKey::ClearSearch => {
                let previous_index = self.focused_visible_index();
                if !self.search.is_empty() {
                    self.search.clear();
                    self.rebuild_visible_rows();
                    self.restore_focus_or_fallback(previous_index);
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
        let previous_index = self.focused_visible_index();
        self.pointer_window_start = None;
        self.request.records = records;
        self.rebuild_visible_rows();
        self.restore_focus_or_fallback(previous_index);
    }

    #[cfg(test)]
    pub(crate) fn focus_visible_session(&mut self, session_id: &str) -> bool {
        self.pointer_window_start = None;
        self.focus_visible_session_in_window(session_id, None)
    }

    pub(crate) fn focus_visible_session_in_window(
        &mut self,
        session_id: &str,
        window_start: Option<usize>,
    ) -> bool {
        if self.visible_index_for_session(session_id).is_none() {
            return false;
        }
        self.pointer_window_start = window_start;
        if self.focused_session_id() != Some(session_id) {
            self.focus = SessionsPickerFocus::SessionId(session_id.to_owned());
        }
        true
    }

    pub(crate) fn focus_start_new(&mut self) {
        self.pointer_window_start = None;
        self.focus = SessionsPickerFocus::StartNew;
    }

    pub(crate) fn focus_start_new_in_window(&mut self, window_start: usize) {
        self.pointer_window_start = Some(window_start);
        self.focus = SessionsPickerFocus::StartNew;
    }

    pub(crate) fn focused_session_id(&self) -> Option<&str> {
        match &self.focus {
            SessionsPickerFocus::StartNew => None,
            SessionsPickerFocus::SessionId(session_id) => Some(session_id.as_str()),
        }
    }

    pub(crate) fn activation_outcome_for_focus(&self) -> Option<SessionsPickerOutcome> {
        match self.focused_session_id() {
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

    pub(super) fn focused_visible_index(&self) -> usize {
        match self.focused_session_id() {
            Some(session_id) => self.visible_index_for_session(session_id).unwrap_or(0),
            None => 0,
        }
    }

    pub(super) fn focused_record(&self) -> Option<&SessionPickerRecord> {
        self.visible_choice_record_at(self.focused_visible_index())
    }

    pub(super) fn focused_window_start(&self, visible_rows: usize) -> usize {
        let visible_len = self.visible_len();
        let maximum_window_start = visible_len.saturating_sub(visible_rows);
        if let Some(pointer_window_start) = self.pointer_window_start {
            let pointer_window_start = pointer_window_start.min(maximum_window_start);
            let focused_index = self.focused_visible_index();
            if focused_index >= pointer_window_start
                && focused_index < pointer_window_start.saturating_add(visible_rows)
            {
                return pointer_window_start;
            }
        }
        visible_window_start(self.focused_visible_index(), visible_len, visible_rows)
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

    fn focus_first_record_when_available(&mut self) {
        if let Some(session_id) = self
            .visible_record_at(0)
            .map(|record| record.session_id.clone())
        {
            self.focus = SessionsPickerFocus::SessionId(session_id);
        }
    }

    fn focus_visible_index(&mut self, index: usize) {
        if index == 0 {
            self.focus_start_new();
            return;
        }
        if let Some(session_id) = self
            .visible_choice_record_at(index)
            .map(|record| record.session_id.clone())
        {
            self.focus = SessionsPickerFocus::SessionId(session_id);
        }
    }

    fn visible_index_for_session(&self, session_id: &str) -> Option<usize> {
        self.visible_indices
            .iter()
            .position(|record_index| {
                self.request
                    .records
                    .get(*record_index)
                    .is_some_and(|record| record.session_id == session_id)
            })
            .map(|index| index + 1)
    }

    fn restore_focus_or_fallback(&mut self, previous_index: usize) {
        if self
            .focused_session_id()
            .is_some_and(|session_id| self.visible_index_for_session(session_id).is_some())
        {
            return;
        }
        self.focus_visible_index(previous_index.min(self.visible_len().saturating_sub(1)));
    }
}

pub(super) fn visible_window_start(
    focused_index: usize,
    visible_len: usize,
    max_visible: usize,
) -> usize {
    if visible_len <= max_visible {
        return 0;
    }
    focused_index
        .saturating_add(1)
        .saturating_sub(max_visible)
        .min(visible_len.saturating_sub(max_visible))
}
