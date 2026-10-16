//! Atomic publication of a source view; a pending switch never relabels retained rows.
use super::{
    picker_machine_controls::PickerMachineStage,
    picker_model::SessionsPickerModel,
    source_reload_progress::{SourceReadState, SourceRecordsUpdate, SourceReloadUpdate},
    source_reload_worker::SessionRecordsReloadRequest,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SourceSwitchCheckpoint {
    switch_request: SessionRecordsReloadRequest,
    prior_model: Box<SessionsPickerModel>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SourceReloadAcceptance {
    Ignored,
    Applied,
    RestoredIndependentView,
    RestoredRetiredView,
}

impl SessionsPickerModel {
    pub(super) fn prepare_source_reload(&mut self, generation: u64) -> SessionRecordsReloadRequest {
        self.source_switch_checkpoint = None;
        let request = SessionRecordsReloadRequest {
            generation,
            query: self.data_query(),
            sources: self.source_contexts(),
        };
        if matches!(
            self.machine_controls.stage,
            PickerMachineStage::Loading { .. }
        ) {
            let mut previous = self.clone();
            previous.machine_controls.cancel_choice();
            self.source_switch_checkpoint = Some(SourceSwitchCheckpoint {
                switch_request: request.clone(),
                prior_model: Box::new(previous),
            });
        }
        request
    }

    pub(super) fn accept_source_reload(
        &mut self,
        request: &SessionRecordsReloadRequest,
        update: SourceReloadUpdate,
    ) -> SourceReloadAcceptance {
        if self.source_action.pending.is_some() {
            return SourceReloadAcceptance::Ignored;
        }
        if self.data_query() != request.query || self.source_contexts() != request.sources {
            return SourceReloadAcceptance::Ignored;
        }
        if matches!(
            self.machine_controls.stage,
            PickerMachineStage::Loading { .. }
        ) {
            match update.records_update {
                SourceRecordsUpdate::Pending => {}
                SourceRecordsUpdate::Ready(records) => {
                    let complete = update.source_progress.iter().all(|progress| {
                        matches!(progress.read_state, SourceReadState::Complete { .. })
                    });
                    if !complete && self.source_switch_checkpoint.is_none() {
                        let mut previous = self.clone();
                        previous.source_switch_checkpoint = None;
                        previous.machine_controls.cancel_choice();
                        self.source_switch_checkpoint = Some(SourceSwitchCheckpoint {
                            switch_request: request.clone(),
                            prior_model: Box::new(previous),
                        });
                    }
                    if complete {
                        self.source_switch_checkpoint = None;
                    }
                    self.machine_controls.complete_source_switch(Ok(()));
                    self.source_progress = update.source_progress;
                    self.replace_records(records);
                }
                SourceRecordsUpdate::Rejected(reason) => {
                    self.machine_controls.complete_source_switch(Err(reason));
                    let failed_choice = self.machine_controls.stage.clone();
                    if let Some(acceptance) = self.restore_switch_checkpoint(request, reason) {
                        self.machine_controls.stage = failed_choice;
                        return acceptance;
                    }
                }
            }
            return SourceReloadAcceptance::Applied;
        }
        self.source_progress = update.source_progress;
        match update.records_update {
            SourceRecordsUpdate::Pending => {}
            SourceRecordsUpdate::Ready(records) => {
                if self
                    .source_progress
                    .iter()
                    .all(|progress| matches!(progress.read_state, SourceReadState::Complete { .. }))
                {
                    self.source_switch_checkpoint = None;
                }
                self.replace_records(records);
            }
            SourceRecordsUpdate::Rejected(reason) => {
                if let Some(acceptance) = self.restore_switch_checkpoint(request, reason) {
                    return acceptance;
                }
                if self.machine_controls.filter
                    != super::picker_machine_controls::PickerMachineFilter::Default
                {
                    self.replace_records(crate::picker_runtime_status::PickerRecordsSnapshot {
                        records: vec![],
                        runtime_coverage:
                            crate::picker_runtime_status::PickerRuntimeCoverage::Unavailable,
                    });
                }
                for progress in &mut self.source_progress {
                    if matches!(progress.read_state, SourceReadState::Loading) {
                        progress.read_state = SourceReadState::Rejected { reason };
                    }
                }
                self.invalidate_runtime_statuses();
            }
        }
        SourceReloadAcceptance::Applied
    }

    fn restore_switch_checkpoint(
        &mut self,
        request: &SessionRecordsReloadRequest,
        reason: super::SourceInventoryRejection,
    ) -> Option<SourceReloadAcceptance> {
        let checkpoint = self.source_switch_checkpoint.take()?;
        if checkpoint.switch_request != *request {
            return None;
        }
        let mut previous = *checkpoint.prior_model;
        let contains_failed = request.sources.iter().any(|failed| {
            previous
                .source_contexts()
                .iter()
                .any(|source| same_inventory_source(source, failed))
        });
        if contains_failed {
            previous.retire_restored_work(&request.sources, reason);
        }
        // A view rollback cannot rewind invocation-local source attempts or their error state.
        previous.source_action = std::mem::take(&mut self.source_action);
        *self = previous;
        Some(if contains_failed {
            SourceReloadAcceptance::RestoredRetiredView
        } else {
            SourceReloadAcceptance::RestoredIndependentView
        })
    }

    fn retire_restored_work(
        &mut self,
        failed: &[super::PickerSourceContext],
        reason: super::SourceInventoryRejection,
    ) {
        for source in self.source_contexts() {
            if !self
                .source_progress
                .iter()
                .any(|progress| progress.source_context == source)
            {
                self.source_progress
                    .push(super::source_reload_progress::SourceReadProgress {
                        source_context: source,
                        read_state: SourceReadState::Rejected {
                            reason: super::SourceInventoryRejection::StaleSnapshot,
                        },
                    });
            }
        }
        for progress in &mut self.source_progress {
            if failed
                .iter()
                .any(|failed| same_inventory_source(&progress.source_context, failed))
            {
                progress.read_state = SourceReadState::Rejected { reason };
            } else if matches!(
                progress.read_state,
                SourceReadState::Loading
                    | SourceReadState::LoadingMore { .. }
                    | SourceReadState::Suspended { .. }
            ) {
                progress.read_state = SourceReadState::Rejected {
                    reason: super::SourceInventoryRejection::StaleSnapshot,
                };
            }
        }
        let records = self
            .request
            .records
            .iter()
            .filter(|record| {
                record.source_context.as_ref().is_some_and(|source| {
                    self.source_progress.iter().any(|progress| {
                        progress.source_context == *source
                            && matches!(progress.read_state, SourceReadState::Complete { .. })
                    })
                })
            })
            .cloned()
            .collect();
        self.replace_records(crate::picker_runtime_status::PickerRecordsSnapshot {
            records,
            runtime_coverage: self.runtime_coverage,
        });
    }
}

fn same_inventory_source(
    first: &super::PickerSourceContext,
    second: &super::PickerSourceContext,
) -> bool {
    match (first, second) {
        (
            super::PickerSourceContext::ConfiguredHosted(first),
            super::PickerSourceContext::ConfiguredHosted(second),
        ) => first.service_id == second.service_id,
        _ => first == second,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::session_picker::{
        PickerSourceContext, SourceInventoryRejection,
        picker_machine_controls::{PickerMachineFilter, PickerMachineSourceMode},
        test_support::{observed_records, picker_record, picker_request},
    };

    fn selected_source() -> PickerSourceContext {
        let registry = crate::sessions::router_connection_registry::RouterConnectionRegistry::parse(
            r#"{"version":1,"routers":[{"name":"Selected source","connection":{"kind":"remote","serviceId":"00000000-0000-4000-8000-000000000001","mcpUrl":"http://127.0.0.1:43129/mcp"}}]}"#,
        ).unwrap();
        PickerSourceContext::ConfiguredHosted(registry.routers[0].clone())
    }

    #[test]
    fn failed_return_to_default_keeps_selected_source_and_rows() {
        let source = selected_source();
        let mut model = SessionsPickerModel::new(picker_request(), 120);
        model.machine_controls.filter = PickerMachineFilter::Single {
            source: Box::new(source.clone()),
        };
        let rows = model.request.records.clone();
        model
            .machine_controls
            .return_to_default(PickerMachineSourceMode::HostedDefault, true);
        assert_eq!(
            model.machine_controls.filter,
            PickerMachineFilter::Single {
                source: Box::new(source)
            }
        );
        let request = SessionRecordsReloadRequest {
            generation: 3,
            query: model.data_query(),
            sources: model.source_contexts(),
        };
        assert_eq!(request.sources, vec![PickerSourceContext::DefaultHosted]);
        model.accept_source_reload(
            &request,
            SourceReloadUpdate {
                source_progress: vec![],
                records_update: SourceRecordsUpdate::Rejected(
                    SourceInventoryRejection::SourceUnavailable,
                ),
            },
        );
        assert!(matches!(
            model.machine_controls.filter,
            PickerMachineFilter::Single { .. }
        ));
        assert_eq!(model.request.records, rows);
    }

    #[test]
    fn returning_to_default_commits_only_its_current_read() {
        let mut model = SessionsPickerModel::new(picker_request(), 120);
        model.machine_controls.filter = PickerMachineFilter::Single {
            source: Box::new(selected_source()),
        };
        model
            .machine_controls
            .return_to_default(PickerMachineSourceMode::HostedDefault, true);
        let request = SessionRecordsReloadRequest {
            generation: 3,
            query: model.data_query(),
            sources: model.source_contexts(),
        };
        model.accept_source_reload(
            &request,
            SourceReloadUpdate {
                source_progress: vec![],
                records_update: SourceRecordsUpdate::Ready(observed_records(vec![picker_record(
                    "default-row",
                    "Fresh default row",
                    "/default",
                    "codex-router",
                    "cli",
                )])),
            },
        );
        assert_eq!(model.machine_controls.filter, PickerMachineFilter::Default);
        assert_eq!(model.request.records.len(), 1);
        assert_eq!(model.request.records[0].session_id, "default-row");
        assert_eq!(
            model.request.records[0].source_context,
            Some(PickerSourceContext::DefaultHosted)
        );
    }

    #[test]
    fn canceled_source_switch_cannot_publish_a_late_reply() {
        let mut model = SessionsPickerModel::new(picker_request(), 120);
        let source = selected_source();
        model.machine_controls.stage = PickerMachineStage::Loading {
            source: Box::new(source.clone()),
            focus: 2,
        };
        let request = SessionRecordsReloadRequest {
            generation: 3,
            query: model.data_query(),
            sources: vec![source],
        };
        let rows = model.request.records.clone();
        model.machine_controls.cancel_choice();
        model.accept_source_reload(
            &request,
            SourceReloadUpdate {
                source_progress: vec![],
                records_update: SourceRecordsUpdate::Ready(observed_records(vec![])),
            },
        );
        assert_eq!(model.machine_controls.filter, PickerMachineFilter::Default);
        assert_eq!(model.request.records, rows);
    }

    #[test]
    fn source_action_view_rollback_keeps_attempts_monotonic_and_rejects_canceled_completion() {
        use crate::presentation::session_picker::{
            picker_machine_controls::MachineChoicePurpose,
            source_action_tests::{prepared, source_model},
        };
        let mut model = source_model();
        let crate::sessions::RouterRegistryRead::Ready(registry) = &model.request.router_registry
        else {
            panic!("fixture registry");
        };
        let source =
            PickerSourceContext::ConfiguredHosted(registry.routers.first().unwrap().clone());
        model.machine_controls.stage = PickerMachineStage::Loading {
            source: Box::new(source),
            focus: 2,
        };
        let reload = model.prepare_source_reload(41);
        assert!(
            model.source_switch_checkpoint.is_some(),
            "checkpoint must precede the action"
        );
        model.machine_controls.stage = PickerMachineStage::Choosing {
            purpose: MachineChoicePurpose::NewSession,
            focus: 1,
            notice: None,
        };
        assert!(model.begin_configured_new());
        let canceled = model.source_action.pending.clone().unwrap();
        model.cancel_source_action();
        model.source_action.error =
            Some(crate::sessions::source_ssh_protocol::SourceReadError::ReadUnavailable);
        assert_eq!(
            model.restore_switch_checkpoint(&reload, SourceInventoryRejection::SourceUnavailable),
            Some(SourceReloadAcceptance::RestoredIndependentView)
        );
        assert_eq!(
            model.source_action.error,
            Some(crate::sessions::source_ssh_protocol::SourceReadError::ReadUnavailable),
            "view restoration must not restore an old source error"
        );
        model.machine_controls.stage = PickerMachineStage::Choosing {
            purpose: MachineChoicePurpose::NewSession,
            focus: 1,
            notice: None,
        };
        assert!(model.begin_configured_new());
        let current = model.source_action.pending.clone().unwrap();
        assert_eq!(current.profile, canceled.profile);
        assert_eq!(current.action, canceled.action);
        assert_ne!(
            current.attempt, canceled.attempt,
            "a view rollback cannot reuse a canceled attempt"
        );
        assert!(
            model
                .complete_source_action(&canceled, Ok(prepared(&canceled)))
                .is_none()
        );
        assert_eq!(
            model.source_action.pending.as_ref(),
            Some(&current),
            "late completion must not consume the current attempt"
        );
        assert!(matches!(
            model.complete_source_action(&current, Ok(prepared(&current))),
            Some(
                crate::presentation::session_picker::SessionsPickerOutcome::PreparedSourceSession(
                    _
                )
            )
        ));
    }
}
