use communication_protocol::{NativeActiveFlag, NativeThreadStatus};

/// Availability of the selected Host's observation, separate from any thread's state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PickerRuntimeCoverage {
    #[default]
    Unobserved,
    Available,
    Unavailable,
    LocalOnly,
}

impl PickerRuntimeCoverage {
    pub(crate) const fn notice(self) -> Option<&'static str> {
        match self {
            Self::Unobserved | Self::Available => None,
            Self::Unavailable => Some("Live status unavailable"),
            Self::LocalOnly => Some("Local mode: no live status"),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PickerRecordsSnapshot {
    pub(crate) records: Vec<crate::sessions::SessionPickerRecord>,
    pub(crate) runtime_coverage: PickerRuntimeCoverage,
}

/// Picker-owned projection of the native runtime states that affect browsing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PickerRuntimeStatus {
    #[default]
    Unknown,
    NotLoaded,
    Blocked,
    Active,
    Idle,
    SystemError,
}

impl PickerRuntimeStatus {
    pub(crate) fn from_native(status: &NativeThreadStatus) -> Self {
        match status {
            NativeThreadStatus::NotLoaded => Self::NotLoaded,
            NativeThreadStatus::Idle => Self::Idle,
            NativeThreadStatus::SystemError => Self::SystemError,
            NativeThreadStatus::Active { active_flags }
                if active_flags.iter().any(|flag| {
                    matches!(
                        flag,
                        NativeActiveFlag::WaitingOnApproval | NativeActiveFlag::WaitingOnUserInput
                    )
                }) =>
            {
                Self::Blocked
            }
            NativeThreadStatus::Active { .. } => Self::Active,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::NotLoaded => "Not loaded",
            Self::Blocked => "Blocked",
            Self::Active => "Active",
            Self::Idle => "Idle",
            Self::SystemError => "Error",
        }
    }

    pub(crate) const fn icon(self) -> &'static str {
        match self {
            Self::Unknown => "?",
            Self::NotLoaded => "◌",
            Self::Blocked => "◆",
            Self::Active => "●",
            Self::Idle => "○",
            Self::SystemError => "!",
        }
    }
}

#[cfg(test)]
mod tests {
    use communication_protocol::{NativeActiveFlag, NativeThreadStatus};

    use super::PickerRuntimeStatus;

    #[test]
    fn native_active_waiting_flags_are_blocked() {
        for flag in [
            NativeActiveFlag::WaitingOnApproval,
            NativeActiveFlag::WaitingOnUserInput,
        ] {
            assert_eq!(
                PickerRuntimeStatus::from_native(&NativeThreadStatus::Active {
                    active_flags: vec![flag],
                }),
                PickerRuntimeStatus::Blocked
            );
        }
    }

    #[test]
    fn native_statuses_keep_unknown_separate_from_idle() {
        assert_eq!(PickerRuntimeStatus::default(), PickerRuntimeStatus::Unknown);
        assert_eq!(
            PickerRuntimeStatus::from_native(&NativeThreadStatus::Active {
                active_flags: Vec::new(),
            }),
            PickerRuntimeStatus::Active
        );
        assert_eq!(
            PickerRuntimeStatus::from_native(&NativeThreadStatus::Idle),
            PickerRuntimeStatus::Idle
        );
        assert_eq!(
            PickerRuntimeStatus::from_native(&NativeThreadStatus::NotLoaded),
            PickerRuntimeStatus::NotLoaded
        );
        assert_eq!(
            PickerRuntimeStatus::from_native(&NativeThreadStatus::SystemError),
            PickerRuntimeStatus::SystemError
        );
    }
}
