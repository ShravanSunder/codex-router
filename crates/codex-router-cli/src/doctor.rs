//! Redacted doctor output.

/// Doctor state for one account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorAccountState {
    label: String,
    quota: QuotaDoctorState,
}

impl DoctorAccountState {
    /// Creates account doctor state.
    #[must_use]
    pub fn new(label: impl Into<String>, quota: QuotaDoctorState) -> Self {
        Self {
            label: label.into(),
            quota,
        }
    }
}

/// Quota state shown by doctor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuotaDoctorState {
    /// Snapshot is fresh.
    Fresh {
        /// Age in seconds.
        age_seconds: u64,
    },
    /// Snapshot is stale. Secret canary exists only to test redaction.
    Stale {
        /// Age in seconds.
        age_seconds: u64,
        /// Secret canary that must never render.
        secret_canary: String,
    },
    /// Snapshot is missing.
    Missing,
}

/// Doctor report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorReport {
    accounts: Vec<DoctorAccountState>,
}

impl DoctorReport {
    /// Creates a doctor report.
    #[must_use]
    pub fn new(accounts: Vec<DoctorAccountState>) -> Self {
        Self { accounts }
    }

    /// Renders redacted report output.
    #[must_use]
    pub fn render(&self) -> String {
        let mut output = String::new();
        for account in &self.accounts {
            output.push_str(account.label.as_str());
            output.push_str(": ");
            match account.quota {
                QuotaDoctorState::Fresh { age_seconds } => {
                    output.push_str("quota: fresh age=");
                    output.push_str(age_seconds.to_string().as_str());
                    output.push_str("s\n");
                }
                QuotaDoctorState::Stale { age_seconds, .. } => {
                    output.push_str("quota: stale age=");
                    output.push_str(age_seconds.to_string().as_str());
                    output.push_str("s\n");
                }
                QuotaDoctorState::Missing => {
                    output.push_str("quota: missing\n");
                }
            }
        }

        output
    }
}
