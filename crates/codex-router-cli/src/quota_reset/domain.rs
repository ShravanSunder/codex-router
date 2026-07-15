//! Pure eligibility and reset-credit selection rules.
#![allow(
    dead_code,
    reason = "shared reset contracts are integrated by later reviewed slices"
)]

/// Active credential version expected by one workflow attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::quota_reset) struct ActiveCredentialGeneration(u64);

impl ActiveCredentialGeneration {
    pub(in crate::quota_reset) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// UI attempt identity used to reject stale completions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::quota_reset) struct AttemptGeneration(u64);

impl AttemptGeneration {
    pub(in crate::quota_reset) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// Unique identity for one operation within an attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::quota_reset) struct OperationGeneration(u64);

impl OperationGeneration {
    pub(in crate::quota_reset) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// Provider redemption identity minted only with commit authority.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(in crate::quota_reset) struct RedeemRequestId(String);

/// The five provider operations surfaced independently in reset detail.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::quota_reset) enum OperationKind {
    InspectionLiveUsage,
    InspectionCreditInventory,
    RevalidationLiveUsage,
    RevalidationCreditInventory,
    ConsumeCredit,
}

impl OperationKind {
    pub(crate) const ALL: [Self; 5] = [
        Self::InspectionLiveUsage,
        Self::InspectionCreditInventory,
        Self::RevalidationLiveUsage,
        Self::RevalidationCreditInventory,
        Self::ConsumeCredit,
    ];
}

/// Sanitized failure classes safe for presentation and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenderSafeFailure {
    AccountUnavailable,
    CredentialGenerationChanged,
    CredentialUnavailable,
    CredentialExpired,
    Transport,
    TimedOut,
    ProviderStatus,
    InvalidResponse,
    EligibilityRefused,
    SelectedCreditChanged,
    Cancelled,
}

impl RenderSafeFailure {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::AccountUnavailable => "account unavailable",
            Self::CredentialGenerationChanged => "credential generation changed",
            Self::CredentialUnavailable => "credential unavailable",
            Self::CredentialExpired => "credential expired",
            Self::Transport => "provider transport unavailable",
            Self::TimedOut => "provider operation timed out",
            Self::ProviderStatus => "provider returned an unsuccessful status",
            Self::InvalidResponse => "provider response was invalid",
            Self::EligibilityRefused => "reset eligibility refused",
            Self::SelectedCreditChanged => "selected reset credit changed",
            Self::Cancelled => "operation cancelled",
        }
    }
}

/// Validated live weekly usage safe to render.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LiveWeeklyUsage {
    remaining_percent: u32,
}

impl LiveWeeklyUsage {
    pub(crate) const fn new(remaining_percent: u32) -> Self {
        Self { remaining_percent }
    }

    pub(crate) const fn remaining_percent(self) -> u32 {
        self.remaining_percent
    }
}

/// Result category returned by a live-usage provider port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LiveUsagePortResult {
    Known(LiveWeeklyUsage),
    Failed(RenderSafeFailure),
}

/// Render-safe summary of a validated complete credit inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CreditInventorySummary {
    pub(crate) credit_count: usize,
    pub(crate) usable_credit_count: usize,
}

/// Result category returned by a credit-inventory provider port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CreditInventoryPortResult {
    Validated(CreditInventorySummary),
    Failed(RenderSafeFailure),
}

/// Validated known provider outcome after consume invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum KnownConsumeOutcome {
    Reset { windows_reset: u32 },
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
}

/// Conservative provider-port classification after the irreversible boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConsumePortResult {
    Known(KnownConsumeOutcome),
    OutcomeUnknown(RenderSafeFailure),
}

/// Provider-reported reset credit used by the guarded reset workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveResetCredit {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) expires_unix_seconds: Option<i64>,
    pub(crate) expires_at: Option<String>,
    pub(crate) title: Option<String>,
}

/// Why a selected account cannot consume a reset credit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResetEligibilityRefusal {
    WeeklyWindowMissing,
    WeeklyRemainingNotBelowOnePercent { remaining_percent: u32 },
    NoAvailableResetCredit,
    SelectedCreditChanged,
}

/// Returns the earliest-expiring available credit after enforcing the live weekly guard.
pub(crate) fn select_guarded_reset_credit(
    weekly_remaining_percent: Option<u32>,
    credits: &[LiveResetCredit],
) -> Result<&LiveResetCredit, ResetEligibilityRefusal> {
    let weekly_remaining_percent =
        weekly_remaining_percent.ok_or(ResetEligibilityRefusal::WeeklyWindowMissing)?;
    if weekly_remaining_percent >= 1 {
        return Err(ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent {
            remaining_percent: weekly_remaining_percent,
        });
    }

    credits
        .iter()
        .filter(|credit| credit.status == "available")
        .min_by_key(|credit| (credit.expires_unix_seconds.unwrap_or(i64::MAX), &credit.id))
        .ok_or(ResetEligibilityRefusal::NoAvailableResetCredit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_weekly_guard_refuses_missing_one_and_higher_percentages() {
        let credit = available_credit("credit-a", Some(100));

        assert_eq!(
            select_guarded_reset_credit(None, std::slice::from_ref(&credit)),
            Err(ResetEligibilityRefusal::WeeklyWindowMissing)
        );
        assert_eq!(
            select_guarded_reset_credit(Some(1), std::slice::from_ref(&credit)),
            Err(ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent {
                remaining_percent: 1
            })
        );
        assert_eq!(
            select_guarded_reset_credit(Some(75), std::slice::from_ref(&credit)),
            Err(ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent {
                remaining_percent: 75
            })
        );
    }

    #[test]
    fn zero_percent_selects_earliest_expiring_available_credit() {
        let credits = vec![
            available_credit("never", None),
            available_credit("later", Some(200)),
            LiveResetCredit {
                id: "redeemed".to_owned(),
                status: "redeemed".to_owned(),
                expires_unix_seconds: Some(50),
                expires_at: Some("1970-01-01T00:00:50Z".to_owned()),
                title: None,
            },
            available_credit("earliest", Some(100)),
        ];

        let selected = select_guarded_reset_credit(Some(0), &credits)
            .unwrap_or_else(|error| panic!("zero percent should be eligible: {error:?}"));

        assert_eq!(selected.id, "earliest");
    }

    #[test]
    fn zero_percent_without_available_credit_fails_closed() {
        let credits = vec![LiveResetCredit {
            id: "redeemed".to_owned(),
            status: "redeemed".to_owned(),
            expires_unix_seconds: Some(50),
            expires_at: Some("1970-01-01T00:00:50Z".to_owned()),
            title: None,
        }];

        assert_eq!(
            select_guarded_reset_credit(Some(0), &credits),
            Err(ResetEligibilityRefusal::NoAvailableResetCredit)
        );
    }

    fn available_credit(id: &str, expires_unix_seconds: Option<i64>) -> LiveResetCredit {
        LiveResetCredit {
            id: id.to_owned(),
            status: "available".to_owned(),
            expires_unix_seconds,
            expires_at: expires_unix_seconds.map(|value| format!("unix-{value}")),
            title: None,
        }
    }
}
