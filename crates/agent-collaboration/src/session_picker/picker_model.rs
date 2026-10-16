use crate::picker_runtime_status::{
    PickerRecordsSnapshot, PickerRuntimeCoverage, PickerRuntimeStatus,
};
use crate::presentation::session_picker::picker_actions::SessionsPickerKey;
use crate::presentation::session_picker::picker_actions::SessionsPickerOutcome;
use crate::presentation::session_picker::picker_filters::next_root_filter;
use crate::presentation::session_picker::picker_filters::next_sort_filter;
use crate::presentation::session_picker::picker_filters::provider_matches;
use crate::presentation::session_picker::picker_filters::root_matches;
use crate::presentation::session_picker::picker_filters::runtime_view_matches;
use crate::presentation::session_picker::picker_filters::source_matches;
#[cfg(test)]
use crate::presentation::session_picker::picker_rendering::render_model_snapshot;
use crate::presentation::session_picker::picker_request::SessionsPickerDataQuery;
use crate::presentation::session_picker::picker_request::SessionsPickerRequest;
use crate::presentation::session_picker::picker_request::SessionsPickerRoot;
use crate::sessions::SessionSearchExpression;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsSort;
use crate::sessions::SessionsSource;
use crate::sessions::{SessionPickerIdentity, SessionPickerRecord};

pub(super) const VISIBLE_SESSION_ROWS: usize = 8;

#[path = "picker_runtime_view.rs"]
mod picker_runtime_view;
pub(crate) use picker_runtime_view::SessionsPickerRuntimeView;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) enum SessionsPickerFocus {
    #[default]
    StartNew,
    Session(SessionPickerIdentity),
}

/// Pure sessions picker state. iocraft owns rendering/input, this owns behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionsPickerModel {
    pub(super) request: SessionsPickerRequest,
    pub(super) width: usize,
    pub(super) machine_controls: super::picker_machine_controls::PickerMachineControls,
    pub(super) fork_confirmation: Option<super::picker_fork_confirmation::ForkConfirmation>,
    pub(super) source_switch_checkpoint: Option<super::picker_source_view::SourceSwitchCheckpoint>,
    pub(super) source_progress: Vec<super::source_reload_progress::SourceReadProgress>,
    pub(super) root: SessionsPickerRoot,
    pub(super) provider: SessionsProvider,
    pub(super) source: SessionsSource,
    pub(crate) runtime_view: SessionsPickerRuntimeView,
    pub(super) sort: SessionsSort,
    pub(super) search: String,
    pub(super) show_help: bool,
    pub(super) source_action: super::source_action_state::PickerSourceAction,
    pub(super) provider_filter: super::provider_filter_model::SessionProviderFilter,
    pub(super) provider_filter_draft: Option<super::provider_filter_model::ProviderFilterDraft>,
    pub(super) details_dialog: Option<super::details_dialog_state::SessionDetailsDialog>,
    pub(super) runtime_coverage: PickerRuntimeCoverage,
    focus: SessionsPickerFocus,
    pointer_window_start: Option<usize>,
    visible_indices: Vec<usize>,
    visible_rows_generation: usize,
}

impl SessionsPickerModel {
    pub(crate) fn new(mut request: SessionsPickerRequest, width: usize) -> Self {
        let default_source = match request.machine_mode {
            super::PickerMachineSourceMode::HostedDefault => {
                super::PickerSourceContext::DefaultHosted
            }
            super::PickerMachineSourceMode::LocalCodex => super::PickerSourceContext::LocalCodex,
        };
        for record in &mut request.records {
            if record.source_context.is_none() {
                record.source_context = Some(default_source.clone());
            }
        }
        let mut model = Self {
            root: request.root,
            provider: request.provider.clone(),
            source: request.source,
            runtime_view: SessionsPickerRuntimeView::All,
            sort: request.sort,
            request,
            width,
            machine_controls: super::picker_machine_controls::PickerMachineControls::default(),
            fork_confirmation: None,
            source_progress: Vec::new(),
            source_switch_checkpoint: None,
            search: String::new(),
            show_help: false,
            source_action: super::source_action_state::PickerSourceAction::default(),
            provider_filter: super::provider_filter_model::SessionProviderFilter::All,
            provider_filter_draft: None,
            details_dialog: None,
            runtime_coverage: PickerRuntimeCoverage::Unobserved,
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
        if self.source_action.pending.is_some() {
            return;
        }
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
            SessionsPickerKey::CycleRuntimeView => {
                let previous_index = self.focused_visible_index();
                self.runtime_view = self.runtime_view.next();
                self.rebuild_visible_rows();
                self.restore_focus_or_fallback(previous_index);
            }
            SessionsPickerKey::CycleSort => {
                let previous_index = self.focused_visible_index();
                self.sort = next_sort_filter(self.sort);
                self.rebuild_visible_rows();
                self.restore_focus_or_fallback(previous_index);
            }
            SessionsPickerKey::ToggleHelp => {
                self.show_help = !self.show_help;
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

    pub(super) fn append_search_text(&mut self, text: &str) {
        if self.source_action.pending.is_some() {
            return;
        }
        let previous_length = self.search.len();
        self.search
            .extend(text.chars().filter(|character| !character.is_control()));
        if self.search.len() == previous_length {
            return;
        }
        let previous_index = self.focused_visible_index();
        self.pointer_window_start = None;
        self.rebuild_visible_rows();
        self.restore_focus_or_fallback(previous_index);
    }

    pub(crate) fn set_width(&mut self, width: usize) {
        self.width = width;
    }

    pub(super) fn open_provider_filters(&mut self) {
        if self.fork_confirmation.is_none()
            && matches!(
                self.machine_controls.stage,
                super::picker_machine_controls::PickerMachineStage::Browsing
            )
            && self.provider_filter_draft.is_none()
            && self.details_dialog.is_none()
        {
            self.provider_filter_draft = Some(
                super::provider_filter_model::ProviderFilterDraft::new(self.provider_filter),
            );
        }
    }

    pub(super) fn apply_provider_filters(&mut self) {
        let Some(draft) = self.provider_filter_draft.take() else {
            return;
        };
        self.provider_filter = draft.selection;
        self.pointer_window_start = None;
        self.rebuild_visible_rows();
        if self
            .focused_identity()
            .is_some_and(|identity| self.visible_index_for_identity(identity).is_none())
        {
            self.focus_start_new();
            self.focus_first_record_when_available();
        }
    }

    pub(super) fn open_details_dialog(&mut self) {
        if self.provider_filter_draft.is_none()
            && self.fork_confirmation.is_none()
            && matches!(
                self.machine_controls.stage,
                super::picker_machine_controls::PickerMachineStage::Browsing
            )
        {
            self.open_captured_details();
        }
    }

    pub(crate) fn data_query(&self) -> SessionsPickerDataQuery {
        SessionsPickerDataQuery {
            root: self.root,
            provider: self.provider.clone(),
            source: self.source,
            sort: self.sort,
            search: self.search.clone(),
            include_empty_sessions: self.request.include_empty_sessions,
        }
    }

    pub(crate) fn replace_records(&mut self, snapshot: PickerRecordsSnapshot) {
        let previous_index = self.focused_visible_index();
        self.pointer_window_start = None;
        self.request.records = snapshot.records;
        let default_source = self.source_contexts().into_iter().next();
        for record in &mut self.request.records {
            if record.source_context.is_none() {
                record.source_context = default_source.clone();
            }
        }
        self.runtime_coverage = snapshot.runtime_coverage;
        self.rebuild_visible_rows();
        self.restore_focus_or_fallback(previous_index);
    }

    pub(crate) fn invalidate_runtime_statuses(&mut self) {
        self.runtime_coverage = PickerRuntimeCoverage::Unavailable;
        if self
            .request
            .records
            .iter()
            .all(|record| record.runtime_status == PickerRuntimeStatus::Unknown)
        {
            return;
        }
        let previous_index = self.focused_visible_index();
        for record in &mut self.request.records {
            record.runtime_status = PickerRuntimeStatus::Unknown;
        }
        self.rebuild_visible_rows();
        self.restore_focus_or_fallback(previous_index);
    }

    #[cfg(test)]
    pub(crate) fn focus_visible_session(&mut self, session_id: &str) -> bool {
        self.pointer_window_start = None;
        let identity = self
            .request
            .records
            .iter()
            .find(|record| record.session_id == session_id)
            .map(|record| record.identity.clone());
        identity.is_some_and(|identity| self.focus_visible_identity_in_window(&identity, None))
    }

    #[cfg(test)]
    pub(crate) fn focus_visible_session_in_window(
        &mut self,
        session_id: &str,
        window_start: Option<usize>,
    ) -> bool {
        let identity = self
            .request
            .records
            .iter()
            .find(|record| record.session_id == session_id)
            .map(|record| record.identity.clone());
        identity
            .is_some_and(|identity| self.focus_visible_identity_in_window(&identity, window_start))
    }

    #[cfg(test)]
    pub(crate) fn focus_visible_identity(&mut self, identity: &SessionPickerIdentity) -> bool {
        self.focus_visible_identity_in_window(identity, None)
    }

    pub(crate) fn focus_visible_identity_in_window(
        &mut self,
        identity: &SessionPickerIdentity,
        window_start: Option<usize>,
    ) -> bool {
        if self.provider_filter_draft.is_some() || self.details_dialog.is_some() {
            return false;
        }
        if self.visible_index_for_identity(identity).is_none() {
            return false;
        }
        self.pointer_window_start = window_start;
        if self.focused_identity() != Some(identity) {
            self.focus = SessionsPickerFocus::Session(identity.clone());
        }
        true
    }

    pub(crate) fn focus_start_new(&mut self) {
        self.pointer_window_start = None;
        self.focus = SessionsPickerFocus::StartNew;
    }

    pub(crate) fn focus_start_new_in_window(&mut self, window_start: usize) {
        if self.provider_filter_draft.is_some() || self.details_dialog.is_some() {
            return;
        }
        self.pointer_window_start = Some(window_start);
        self.focus = SessionsPickerFocus::StartNew;
    }

    pub(crate) fn focused_session_id(&self) -> Option<&str> {
        self.focused_record()
            .map(|record| record.session_id.as_str())
    }

    pub(crate) fn focused_identity(&self) -> Option<&SessionPickerIdentity> {
        match &self.focus {
            SessionsPickerFocus::StartNew => None,
            SessionsPickerFocus::Session(identity) => Some(identity),
        }
    }

    pub(super) fn source_contexts(&self) -> Vec<super::PickerSourceContext> {
        if let super::picker_machine_controls::PickerMachineStage::Loading { source, .. } =
            &self.machine_controls.stage
        {
            return vec![source.as_ref().clone()];
        }
        if let super::picker_machine_controls::PickerMachineFilter::Single { source } =
            &self.machine_controls.filter
        {
            return vec![source.as_ref().clone()];
        }
        let default_source = match self.request.machine_mode {
            super::PickerMachineSourceMode::HostedDefault => {
                super::PickerSourceContext::DefaultHosted
            }
            super::PickerMachineSourceMode::LocalCodex => super::PickerSourceContext::LocalCodex,
        };
        let mut sources = vec![default_source];
        if self.machine_controls.filter == super::picker_machine_controls::PickerMachineFilter::All
            && let crate::sessions::RouterRegistryRead::Ready(registry) =
                &self.request.router_registry
        {
            sources.extend(
                registry
                    .routers
                    .iter()
                    .cloned()
                    .map(super::PickerSourceContext::ConfiguredHosted),
            );
        }
        sources
    }

    pub(super) fn start_new_action(&mut self) -> Option<SessionsPickerOutcome> {
        if self.provider_filter_draft.is_some() || self.details_dialog.is_some() {
            return None;
        }
        if self
            .machine_controls
            .open_new(&self.request.router_registry)
        {
            None
        } else {
            Some(SessionsPickerOutcome::StartNewSession)
        }
    }

    pub(crate) fn activation_outcome_for_focus(&self) -> Option<SessionsPickerOutcome> {
        match self.focused_record() {
            Some(record) if record.identity.is_provider() => None,
            Some(record) => Some(SessionsPickerOutcome::ResumeSession(
                crate::sessions::SessionActionSelection::from_picker_record(record),
            )),
            None => Some(SessionsPickerOutcome::StartNewSession),
        }
    }

    #[cfg(test)]
    pub(crate) fn fork_outcome_for_focus(&self) -> Option<SessionsPickerOutcome> {
        self.focused_record()
            .filter(|record| !record.identity.is_provider())
            .map(|record| {
                SessionsPickerOutcome::ForkSession(
                    crate::sessions::SessionActionSelection::from_picker_record(record),
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn render_snapshot(&self) -> String {
        render_model_snapshot(self)
    }

    pub(super) fn open_fork_confirmation(&mut self) {
        self.fork_confirmation = self.focused_record().map(|record| {
            super::picker_fork_confirmation::ForkConfirmation::capture(record, &self.request)
        });
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
        match self.focused_identity() {
            Some(identity) => self.visible_index_for_identity(identity).unwrap_or(0),
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
        let search = SessionSearchExpression::parse(&self.search);
        let mut indices = self
            .request
            .records
            .iter()
            .enumerate()
            .filter(|(_index, record)| root_matches(self.root, &self.request, record))
            .filter(|(_index, record)| provider_matches(&self.provider, &self.request, record))
            .filter(|(_index, record)| source_matches(self.source, record))
            .filter(|(_index, record)| runtime_view_matches(self.runtime_view, record))
            .filter(|(_index, record)| {
                self.provider_filter
                    .matches(record.client_provider_family())
            })
            .filter(|(_index, record)| search.is_empty() || record.matches_search(&search))
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
        if let Some(identity) = self
            .visible_record_at(0)
            .map(|record| record.identity.clone())
        {
            self.focus = SessionsPickerFocus::Session(identity);
        }
    }

    fn focus_visible_index(&mut self, index: usize) {
        if index == 0 {
            self.focus_start_new();
            return;
        }
        if let Some(identity) = self
            .visible_choice_record_at(index)
            .map(|record| record.identity.clone())
        {
            self.focus = SessionsPickerFocus::Session(identity);
        }
    }

    fn visible_index_for_identity(&self, identity: &SessionPickerIdentity) -> Option<usize> {
        self.visible_indices
            .iter()
            .position(|record_index| {
                self.request
                    .records
                    .get(*record_index)
                    .is_some_and(|record| &record.identity == identity)
            })
            .map(|index| index + 1)
    }

    fn restore_focus_or_fallback(&mut self, previous_index: usize) {
        if self
            .focused_identity()
            .is_some_and(|identity| self.visible_index_for_identity(identity).is_some())
        {
            return;
        }
        self.focus_visible_index(previous_index.min(self.visible_len().saturating_sub(1)));
    }
}

#[path = "picker_visible_window.rs"]
mod picker_visible_window;
pub(super) use picker_visible_window::visible_window_start;
