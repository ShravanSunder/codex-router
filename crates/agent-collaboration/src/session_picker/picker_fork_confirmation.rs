//! Frozen fork source and destination choices; confirmation never re-reads focus.
use super::{PickerSourceContext, SessionsPickerOutcome, SessionsPickerRequest};
use crate::sessions::{RouterRegistryRead, SessionActionSelection, SessionPickerRecord};
use codex_native_integration::{NativeWorkingDirectoryMetadata, native_working_directory_metadata};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ForkAvailability {
    Eligible,
    ProviderUnsupported,
    SourceUnqualified,
    HistoryRemainsOnSource,
}

impl ForkAvailability {
    pub(super) fn explanation(&self) -> Option<&'static str> {
        match self {
            Self::Eligible => None,
            Self::ProviderUnsupported => Some("Provider fork is unavailable in this launcher."),
            Self::SourceUnqualified => {
                Some("The source connection needs qualification before forking.")
            }
            Self::HistoryRemainsOnSource => {
                Some("History remains on the source machine; cross-machine fork is unavailable.")
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ForkDestination {
    pub(super) label: String,
    pub(super) availability: ForkAvailability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ForkConfirmation {
    pub(super) selection: SessionActionSelection,
    pub(super) title: String,
    pub(super) source_label: String,
    pub(super) directory_label: String,
    pub(super) directory_origin: &'static str,
    pub(super) destinations: Vec<ForkDestination>,
    pub(super) focused_destination: usize,
    pub(super) notice: Option<&'static str>,
}

impl ForkConfirmation {
    pub(super) fn capture(record: &SessionPickerRecord, request: &SessionsPickerRequest) -> Self {
        let source_label = record.machine_label().to_owned();
        let configured = matches!(
            record.source_context,
            Some(PickerSourceContext::ConfiguredHosted(_))
        );
        let (directory_label, directory_origin) = if configured {
            (
                record
                    .cwd
                    .clone()
                    .unwrap_or_else(|| "Unavailable".to_owned()),
                "source session",
            )
        } else {
            match native_working_directory_metadata(&request.current_dir, &request.native_arguments)
            {
                NativeWorkingDirectoryMetadata::Invoking { directory } => {
                    (directory.display().to_string(), "invoking directory")
                }
                NativeWorkingDirectoryMetadata::Explicit { directory } => (
                    directory.to_string_lossy().into_owned(),
                    "explicit native argument",
                ),
                NativeWorkingDirectoryMetadata::UnresolvedExplicit => (
                    "Unresolved explicit override".to_owned(),
                    "native arguments; launcher determines the result",
                ),
            }
        };
        let availability = if record.identity.is_provider() {
            ForkAvailability::ProviderUnsupported
        } else if configured {
            ForkAvailability::SourceUnqualified
        } else {
            ForkAvailability::Eligible
        };
        let mut destinations = vec![ForkDestination {
            label: source_label.clone(),
            availability,
        }];
        if let RouterRegistryRead::Ready(registry) = &request.router_registry {
            destinations.extend(
                registry
                    .routers
                    .iter()
                    .filter(|profile| profile.name.as_str() != source_label)
                    .map(|profile| ForkDestination {
                        label: profile.name.as_str().to_owned(),
                        availability: ForkAvailability::HistoryRemainsOnSource,
                    }),
            );
        }
        Self {
            selection: SessionActionSelection::from_picker_record(record),
            title: record.title.clone(),
            source_label,
            directory_label,
            directory_origin,
            destinations,
            focused_destination: 0,
            notice: None,
        }
    }

    pub(super) fn move_destination(&mut self, delta: isize) {
        self.focused_destination = self
            .focused_destination
            .saturating_add_signed(delta)
            .min(self.destinations.len().saturating_sub(1));
        self.notice = None;
    }

    pub(super) fn confirm(&mut self) -> Option<SessionsPickerOutcome> {
        let destination = self.destinations.get(self.focused_destination)?;
        if let Some(reason) = destination.availability.explanation() {
            self.notice = Some(reason);
            None
        } else {
            Some(SessionsPickerOutcome::ForkSession(self.selection.clone()))
        }
    }
}
