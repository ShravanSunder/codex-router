//! Atomic publication of a source view; a pending switch never relabels retained rows.
use super::{
    picker_machine_controls::PickerMachineStage,
    picker_model::SessionsPickerModel,
    source_reload_progress::{SourceReadState, SourceRecordsUpdate, SourceReloadUpdate},
    source_reload_worker::SessionRecordsReloadRequest,
};

impl SessionsPickerModel {
    pub(super) fn accept_source_reload(
        &mut self,
        request: &SessionRecordsReloadRequest,
        update: SourceReloadUpdate,
    ) {
        if self.data_query() != request.query || self.source_contexts() != request.sources {
            return;
        }
        if matches!(
            self.machine_controls.stage,
            PickerMachineStage::Loading { .. }
        ) {
            match update.records_update {
                SourceRecordsUpdate::Pending => {}
                SourceRecordsUpdate::Ready(records) => {
                    self.machine_controls.complete_source_switch(Ok(()));
                    self.source_progress = update.source_progress;
                    self.replace_records(records);
                }
                SourceRecordsUpdate::Rejected(reason) => {
                    self.machine_controls.complete_source_switch(Err(reason));
                }
            }
            return;
        }
        self.source_progress = update.source_progress;
        match update.records_update {
            SourceRecordsUpdate::Pending => {}
            SourceRecordsUpdate::Ready(records) => self.replace_records(records),
            SourceRecordsUpdate::Rejected(reason) => {
                for progress in &mut self.source_progress {
                    if matches!(progress.read_state, SourceReadState::Loading) {
                        progress.read_state = SourceReadState::Rejected { reason };
                    }
                }
                self.invalidate_runtime_statuses();
            }
        }
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
}
