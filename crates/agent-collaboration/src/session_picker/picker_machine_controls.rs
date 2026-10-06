//! Invocation-local machine browsing and NEW choice; configured routes require qualification.
use crate::sessions::{RouterRegistryError, RouterRegistryRead};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PickerMachineSourceMode {
    #[default]
    HostedDefault,
    LocalCodex,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum PickerMachineFilter {
    #[default]
    Default,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MachineChoicePurpose {
    Browse,
    NewSession,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) enum PickerMachineStage {
    #[default]
    Browsing,
    Choosing {
        purpose: MachineChoicePurpose,
        focus: usize,
        notice: Option<MachineChoiceNotice>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum MachineChoiceNotice {
    Registry(RouterRegistryError),
    SourceNotQualified(String),
    MissingDestinationCwd(String),
    NativeRouteUnavailable(String),
    NativeCredentialTransportUnsupported(String),
    LaunchNotQualified(String),
}

impl MachineChoiceNotice {
    pub(super) fn message(&self) -> String {
        match self {
            Self::Registry(error) => error.to_string(),
            Self::SourceNotQualified(name) => {
                format!("{name}: machine connection needs verification; previous view retained")
            }
            Self::MissingDestinationCwd(name) => format!(
                "{name}: configure a destination working directory before creating a session"
            ),
            Self::NativeRouteUnavailable(name) => {
                format!("{name}: interactive session route is unavailable")
            }
            Self::NativeCredentialTransportUnsupported(name) => {
                format!("{name}: credential reference is unsupported by this native transport")
            }
            Self::LaunchNotQualified(name) => {
                format!("{name}: execution route needs verification; no session created")
            }
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct PickerMachineControls {
    pub(super) filter: PickerMachineFilter,
    pub(super) stage: PickerMachineStage,
    pub(super) control_focused: bool,
}

impl PickerMachineControls {
    pub(super) fn label(&self, mode: PickerMachineSourceMode) -> &'static str {
        match (mode, self.filter) {
            (PickerMachineSourceMode::LocalCodex, _) => "Local Codex",
            (_, PickerMachineFilter::Default) => "This machine",
            (_, PickerMachineFilter::All) => "All machines",
        }
    }

    pub(super) fn open_browse(&mut self, registry: &RouterRegistryRead) {
        self.stage = PickerMachineStage::Choosing {
            purpose: MachineChoicePurpose::Browse,
            focus: 0,
            notice: registry_notice(registry),
        };
    }

    pub(super) fn open_new(&mut self, registry: &RouterRegistryRead) -> bool {
        let needs_choice = match registry {
            RouterRegistryRead::Missing => false,
            RouterRegistryRead::Ready(registry) => !registry.routers.is_empty(),
            RouterRegistryRead::Rejected(_) => true,
        };
        if needs_choice {
            self.stage = PickerMachineStage::Choosing {
                purpose: MachineChoicePurpose::NewSession,
                focus: 0,
                notice: registry_notice(registry),
            };
        }
        needs_choice
    }

    pub(super) fn choices(
        &self,
        registry: &RouterRegistryRead,
        mode: PickerMachineSourceMode,
    ) -> Vec<String> {
        let mut choices = vec![match mode {
            PickerMachineSourceMode::HostedDefault => "This machine (current/default)".to_owned(),
            PickerMachineSourceMode::LocalCodex => "Local Codex".to_owned(),
        }];
        let PickerMachineStage::Choosing { purpose, .. } = self.stage else {
            return choices;
        };
        if mode == PickerMachineSourceMode::HostedDefault {
            if matches!(purpose, MachineChoicePurpose::Browse)
                && matches!(registry, RouterRegistryRead::Ready(registry) if !registry.routers.is_empty())
            {
                choices.push("All machines".to_owned());
            }
            if let RouterRegistryRead::Ready(registry) = registry {
                choices.extend(
                    registry
                        .routers
                        .iter()
                        .map(|profile| profile.name.as_str().to_owned()),
                );
            }
        }
        choices
    }

    pub(super) fn move_choice(&mut self, direction: isize, choice_count: usize) {
        if let PickerMachineStage::Choosing { focus, notice, .. } = &mut self.stage {
            *focus = focus
                .saturating_add_signed(direction)
                .min(choice_count.saturating_sub(1));
            *notice = None;
        }
    }

    pub(super) fn cancel_choice(&mut self) {
        self.stage = PickerMachineStage::Browsing;
        self.control_focused = false;
    }

    /// Returns true only for a concrete, established default NEW action.
    pub(super) fn select_choice(
        &mut self,
        registry: &RouterRegistryRead,
        mode: PickerMachineSourceMode,
    ) -> bool {
        let PickerMachineStage::Choosing { purpose, focus, .. } = self.stage else {
            return false;
        };
        if focus == 0 {
            self.filter = PickerMachineFilter::Default;
            self.cancel_choice();
            return matches!(purpose, MachineChoicePurpose::NewSession);
        }
        let has_all = mode == PickerMachineSourceMode::HostedDefault
            && matches!(purpose, MachineChoicePurpose::Browse)
            && matches!(registry, RouterRegistryRead::Ready(registry) if !registry.routers.is_empty());
        if has_all && focus == 1 {
            self.filter = PickerMachineFilter::All;
            self.cancel_choice();
            return false;
        }
        let profile_index = focus.saturating_sub(if has_all { 2 } else { 1 });
        let RouterRegistryRead::Ready(registry) = registry else {
            return false;
        };
        let Some(profile) = registry.routers.get(profile_index) else {
            return false;
        };
        let name = profile.name.as_str().to_owned();
        let notice = match purpose {
            MachineChoicePurpose::Browse => MachineChoiceNotice::SourceNotQualified(name),
            MachineChoicePurpose::NewSession if profile.default_remote_cwd.is_none() => {
                MachineChoiceNotice::MissingDestinationCwd(name)
            }
            MachineChoicePurpose::NewSession => match &profile.native_codex {
                None => MachineChoiceNotice::NativeRouteUnavailable(name),
                Some(native)
                    if native.credential.is_some()
                        && !native.address.allows_credential_reference() =>
                {
                    MachineChoiceNotice::NativeCredentialTransportUnsupported(name)
                }
                Some(_) => MachineChoiceNotice::LaunchNotQualified(name),
            },
        };
        self.stage = PickerMachineStage::Choosing {
            purpose,
            focus,
            notice: Some(notice),
        };
        false
    }

    pub(super) fn all_notice(&self, registry: &RouterRegistryRead) -> Option<String> {
        if self.filter != PickerMachineFilter::All {
            return None;
        }
        let RouterRegistryRead::Ready(registry) = registry else {
            return None;
        };
        let names = registry
            .routers
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>();
        (!names.is_empty()).then(|| {
            format!(
                "Partial view — unavailable: {} (connections need verification)",
                names.join(", ")
            )
        })
    }
}

fn registry_notice(registry: &RouterRegistryRead) -> Option<MachineChoiceNotice> {
    match registry {
        RouterRegistryRead::Rejected(error) => Some(MachineChoiceNotice::Registry(*error)),
        _ => None,
    }
}
