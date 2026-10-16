//! One immutable read-only source preparation; picker focus cannot retarget it.
use super::{
    PickerSourceContext, SessionsPickerOutcome,
    picker_machine_controls::{MachineChoicePurpose, PickerMachineStage},
    picker_model::SessionsPickerModel,
};
use crate::sessions::{
    RouterRegistryRead, SessionActionSelection, SessionPickerIdentity, SessionRowProvenance,
    router_connection_registry::{RouterConnectionProfile, RouterConnectionTransport},
    source_ssh_protocol::{
        PreparedSourceLaunch, PreparedSourceSession, SourceNativeOptions, SourceReadError,
        SourceRejection, SourceSessionAction, SourceStoredModel,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingSourceAction {
    pub(super) attempt: u64,
    pub(super) profile: RouterConnectionProfile,
    pub(super) action: SourceSessionAction,
    pub(super) source_directory: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct PickerSourceAction {
    pub(super) pending: Option<PendingSourceAction>,
    pub(super) error: Option<SourceReadError>,
    next_attempt: u64,
}

impl SessionsPickerModel {
    fn begin_source_action(
        &mut self,
        profile: RouterConnectionProfile,
        action: SourceSessionAction,
        source_directory: Option<String>,
    ) {
        if self.source_action.pending.is_some() {
            return;
        }
        self.source_action.error = None;
        if let Err(reason) = SourceNativeOptions::parse(&self.request.native_arguments)
            .and_then(|_| action.validate(&profile.service_id))
        {
            self.source_action.error = Some(SourceReadError::Rejected(reason));
            return;
        }
        let Some(attempt) = self.source_action.next_attempt.checked_add(1) else {
            self.source_action.error = Some(SourceReadError::Rejected(
                SourceRejection::SourceUnavailable,
            ));
            return;
        };
        self.source_action.next_attempt = attempt;
        self.show_help = false;
        self.source_action.pending = Some(PendingSourceAction {
            attempt,
            profile,
            action,
            source_directory,
        });
    }

    pub(super) fn begin_configured_new(&mut self) -> bool {
        if self.source_action.pending.is_some() {
            return true;
        }
        let PickerMachineStage::Choosing {
            purpose: MachineChoicePurpose::NewSession,
            focus,
            ..
        } = self.machine_controls.stage
        else {
            return false;
        };
        if focus == 0 {
            return false;
        }
        let RouterRegistryRead::Ready(registry) = &self.request.router_registry else {
            return false;
        };
        let Some(profile) = registry.routers.get(focus - 1).cloned() else {
            return false;
        };
        if !matches!(profile.transport, RouterConnectionTransport::Ssh(_)) {
            return false;
        }
        let Some(directory) = profile.default_remote_cwd.clone() else {
            return false;
        };
        let source_directory = Some(directory.as_str().to_owned());
        self.begin_source_action(
            profile,
            SourceSessionAction::New { directory },
            source_directory,
        );
        true
    }

    pub(super) fn begin_source_selection(
        &mut self,
        selection: &SessionActionSelection,
        fork: bool,
        source_directory: Option<String>,
    ) -> bool {
        let Some(PickerSourceContext::ConfiguredHosted(profile)) = &selection.source_context else {
            return false;
        };
        if !matches!(profile.transport, RouterConnectionTransport::Ssh(_)) {
            return false;
        }
        let SessionPickerIdentity::HostedCodex(target) = &selection.identity else {
            self.source_action.error = Some(SourceReadError::Rejected(
                SourceRejection::SessionUnavailable,
            ));
            return true;
        };
        if selection.provenance != SessionRowProvenance::ObservedHosted
            || target.endpoint.service_id != profile.service_id
        {
            self.source_action.error = Some(SourceReadError::Rejected(
                SourceRejection::SessionUnavailable,
            ));
            return true;
        }
        let stored_model = SourceStoredModel::from_choice(&selection.model_choice);
        let action = if fork {
            SourceSessionAction::Fork {
                target: target.clone(),
                stored_model,
            }
        } else {
            SourceSessionAction::Resume {
                target: target.clone(),
                stored_model,
            }
        };
        self.begin_source_action(profile.clone(), action, source_directory);
        true
    }

    pub(super) fn begin_source_resume(&mut self) -> bool {
        if self.source_action.pending.is_some() {
            return true;
        }
        let Some(record) = self.focused_record() else {
            return false;
        };
        if record.identity.is_provider() {
            return false;
        }
        let source_directory = record.cwd.clone();
        let selection = SessionActionSelection::from_picker_record(record);
        self.begin_source_selection(&selection, false, source_directory)
    }

    pub(super) fn begin_source_fork(&mut self) -> bool {
        if self.source_action.pending.is_some() {
            return true;
        }
        let Some(confirmation) = &self.fork_confirmation else {
            return false;
        };
        if confirmation.focused_destination != 0
            || confirmation
                .destinations
                .first()
                .is_none_or(|destination| destination.availability.explanation().is_some())
        {
            return false;
        }
        let selection = confirmation.selection.clone();
        let source_directory = Some(confirmation.directory_label.clone());
        self.begin_source_selection(&selection, true, source_directory)
    }

    pub(super) fn cancel_source_action(&mut self) {
        self.source_action.pending = None;
        self.source_action.error = None;
    }

    pub(super) fn complete_source_action(
        &mut self,
        captured: &PendingSourceAction,
        result: Result<PreparedSourceLaunch, SourceReadError>,
    ) -> Option<SessionsPickerOutcome> {
        if self.source_action.pending.as_ref() != Some(captured) {
            return None;
        }
        self.source_action.pending = None;
        match result {
            Ok(launch)
                if launch.action == captured.action
                    && launch.endpoint.service_id == captured.profile.service_id
                    && captured
                        .action
                        .target()
                        .is_none_or(|target| target.endpoint == launch.endpoint)
                    && matches!(&captured.profile.transport, RouterConnectionTransport::Ssh(source) if source.client_profile == launch.client_profile) =>
            {
                if matches!(captured.action, SourceSessionAction::Fork { .. }) {
                    let confirmation = self.fork_confirmation.as_mut()?;
                    if captured.source_directory.as_deref()
                        != Some(launch.working_directory.as_str())
                    {
                        confirmation.directory_label = launch.working_directory.as_str().to_owned();
                        confirmation.directory_origin = "verified source session";
                        confirmation.notice =
                            Some("Source directory changed; confirm the verified directory.");
                        return None;
                    }
                }
                Some(SessionsPickerOutcome::PreparedSourceSession(Box::new(
                    PreparedSourceSession {
                        profile: captured.profile.clone(),
                        launch,
                    },
                )))
            }
            Ok(_) => {
                self.source_action.error = Some(SourceReadError::InvalidReply);
                None
            }
            Err(reason) => {
                self.source_action.error = Some(reason);
                None
            }
        }
    }
}
