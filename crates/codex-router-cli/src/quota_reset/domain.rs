//! Pure eligibility and reset-credit selection rules.
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

impl RedeemRequestId {
    const MAXIMUM_LENGTH: usize = 256;

    pub(in crate::quota_reset) fn new(value: String) -> Result<Self, RedeemRequestIdError> {
        if value.is_empty()
            || value.len() > Self::MAXIMUM_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(RedeemRequestIdError);
        }
        Ok(Self(value))
    }

    pub(in crate::quota_reset) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) struct RedeemRequestIdError;

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

/// Result category returned by a credit-inventory provider port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CreditInventoryPortResult {
    Validated(ValidatedCreditInventory),
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
    OutcomeUnknown(ConsumeUnknownReason),
}

/// Allowlisted ambiguous outcomes legal only after consume invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConsumeUnknownReason {
    Transport,
    TimedOut,
    ProviderStatus,
    InvalidResponse,
}

impl ConsumeUnknownReason {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Transport => "consume transport outcome unavailable",
            Self::TimedOut => "consume request timed out after dispatch",
            Self::ProviderStatus => "consume provider status was not definitive",
            Self::InvalidResponse => "consume response was not definitive",
        }
    }
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

/// Complete validated reset-credit inventory in deterministic display order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidatedCreditInventory {
    credits: Vec<ValidatedResetCredit>,
    earliest_usable_index: Option<usize>,
    usable_credit_count: usize,
}

impl ValidatedCreditInventory {
    pub(crate) fn len(&self) -> usize {
        self.credits.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.credits.is_empty()
    }

    pub(crate) fn credit_ids(&self) -> Vec<&str> {
        self.credits
            .iter()
            .map(|credit| credit.identity.0.as_str())
            .collect()
    }

    pub(crate) fn earliest_usable_credit_id(&self) -> Option<&str> {
        self.earliest_usable_index
            .and_then(|index| self.credits.get(index))
            .map(|credit| credit.identity.0.as_str())
    }

    pub(crate) const fn usable_credit_count(&self) -> usize {
        self.usable_credit_count
    }

    pub(crate) fn earliest_usable_expiration(&self) -> Option<Option<i64>> {
        self.earliest_usable_index
            .and_then(|index| self.credits.get(index))
            .map(|credit| credit.expires_unix_seconds)
    }

    pub(in crate::quota_reset) fn earliest_usable_identity(&self) -> Option<ResetCreditIdentity> {
        self.earliest_usable_index
            .and_then(|index| self.credits.get(index))
            .map(|credit| credit.identity.clone())
    }

    pub(in crate::quota_reset) fn earliest_usable_snapshot(
        &self,
    ) -> Option<SelectedResetCreditSnapshot> {
        self.earliest_usable_index
            .and_then(|index| self.credits.get(index))
            .map(SelectedResetCreditSnapshot::from)
    }
}

/// Exact credit identity kept opaque outside reset authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) struct ResetCreditIdentity(String);

impl ResetCreditIdentity {
    pub(in crate::quota_reset) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValidatedCreditStatus {
    Available,
    Redeeming,
    Redeemed,
}

/// Fully validated inventory entry; no raw provider status or timestamp remains.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ValidatedResetCredit {
    identity: ResetCreditIdentity,
    status: ValidatedCreditStatus,
    expires_unix_seconds: Option<i64>,
    title: Option<String>,
}

/// Exact selected-credit fields bound into confirmation and commit authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) struct SelectedResetCreditSnapshot {
    identity: ResetCreditIdentity,
    status: ValidatedCreditStatus,
    expires_unix_seconds: Option<i64>,
    title: Option<String>,
}

impl SelectedResetCreditSnapshot {
    pub(in crate::quota_reset) fn id(&self) -> &str {
        self.identity.as_str()
    }

    pub(in crate::quota_reset) const fn expires_unix_seconds(&self) -> Option<i64> {
        self.expires_unix_seconds
    }
}

impl From<&ValidatedResetCredit> for SelectedResetCreditSnapshot {
    fn from(credit: &ValidatedResetCredit) -> Self {
        Self {
            identity: credit.identity.clone(),
            status: credit.status,
            expires_unix_seconds: credit.expires_unix_seconds,
            title: credit.title.clone(),
        }
    }
}

/// Fail-closed inventory validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreditInventoryValidationError {
    EmptyIdentifier,
    UnsafeText,
    UnknownStatus,
    InvalidExpiration,
}

/// Validates the complete inventory before ordering or selecting any credit.
pub(crate) fn validate_credit_inventory(
    mut credits: Vec<LiveResetCredit>,
    now_unix_seconds: i64,
) -> Result<ValidatedCreditInventory, CreditInventoryValidationError> {
    let mut credits = credits
        .drain(..)
        .map(validate_credit)
        .collect::<Result<Vec<_>, _>>()?;
    credits.sort_by(|left, right| {
        left.expires_unix_seconds
            .is_none()
            .cmp(&right.expires_unix_seconds.is_none())
            .then_with(|| left.expires_unix_seconds.cmp(&right.expires_unix_seconds))
            .then_with(|| left.identity.0.cmp(&right.identity.0))
    });
    let earliest_usable_index = credits
        .iter()
        .position(|credit| credit_is_usable(credit, now_unix_seconds));
    let usable_credit_count = credits
        .iter()
        .filter(|credit| credit_is_usable(credit, now_unix_seconds))
        .count();
    Ok(ValidatedCreditInventory {
        credits,
        earliest_usable_index,
        usable_credit_count,
    })
}

fn validate_credit(
    credit: LiveResetCredit,
) -> Result<ValidatedResetCredit, CreditInventoryValidationError> {
    if credit.id.trim().is_empty() {
        return Err(CreditInventoryValidationError::EmptyIdentifier);
    }
    if contains_unsafe_text(&credit.id)
        || contains_unsafe_text(&credit.status)
        || credit.title.as_deref().is_some_and(contains_unsafe_text)
    {
        return Err(CreditInventoryValidationError::UnsafeText);
    }
    let status = match credit.status.as_str() {
        "available" => ValidatedCreditStatus::Available,
        "redeeming" => ValidatedCreditStatus::Redeeming,
        "redeemed" => ValidatedCreditStatus::Redeemed,
        _ => return Err(CreditInventoryValidationError::UnknownStatus),
    };
    if credit.expires_at.is_some() != credit.expires_unix_seconds.is_some()
        || credit
            .expires_unix_seconds
            .is_some_and(|expiration| expiration < 0)
    {
        return Err(CreditInventoryValidationError::InvalidExpiration);
    }
    Ok(ValidatedResetCredit {
        identity: ResetCreditIdentity(credit.id),
        status,
        expires_unix_seconds: credit.expires_unix_seconds,
        title: credit.title,
    })
}

fn contains_unsafe_text(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn credit_is_usable(credit: &ValidatedResetCredit, now_unix_seconds: i64) -> bool {
    credit.status == ValidatedCreditStatus::Available
        && credit
            .expires_unix_seconds
            .is_none_or(|expiration| expiration > now_unix_seconds)
}

/// Visible half-open inventory range and total count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InventoryPage {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) total: usize,
}

impl InventoryPage {
    pub(crate) const fn new(start: usize, end: usize, total: usize) -> Self {
        Self { start, end, total }
    }

    pub(crate) const fn remaining(self) -> usize {
        self.total.saturating_sub(self.end)
    }
}

/// Returns a clamped deterministic page aligned to `page_size`.
pub(crate) fn inventory_page(
    total: usize,
    requested_start: usize,
    page_size: usize,
) -> InventoryPage {
    if total == 0 || page_size == 0 {
        return InventoryPage::new(0, 0, total);
    }
    let last_page_start = ((total - 1) / page_size) * page_size;
    let start = requested_start.min(last_page_start) / page_size * page_size;
    InventoryPage::new(start, (start + page_size).min(total), total)
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

    #[test]
    fn inventory_validation_orders_complete_inventory_and_selects_earliest_usable() {
        let credits = vec![
            available_credit("never", None),
            available_credit("later", Some(300)),
            available_credit("expired", Some(50)),
            available_credit("earliest", Some(200)),
            LiveResetCredit {
                id: "redeemed".to_owned(),
                status: "redeemed".to_owned(),
                expires_unix_seconds: Some(100),
                expires_at: Some("unix-100".to_owned()),
                title: None,
            },
        ];

        let inventory = validate_credit_inventory(credits, 100)
            .unwrap_or_else(|error| panic!("inventory should validate: {error:?}"));

        assert_eq!(inventory.len(), 5);
        assert_eq!(
            inventory.credit_ids(),
            ["expired", "redeemed", "earliest", "later", "never"]
        );
        assert_eq!(inventory.earliest_usable_credit_id(), Some("earliest"));
    }

    #[test]
    fn inventory_validation_fails_closed_for_any_malformed_or_unknown_credit() {
        for credit in [
            LiveResetCredit {
                id: String::new(),
                status: "available".to_owned(),
                expires_unix_seconds: None,
                expires_at: None,
                title: None,
            },
            LiveResetCredit {
                id: "unknown".to_owned(),
                status: "future".to_owned(),
                expires_unix_seconds: None,
                expires_at: None,
                title: None,
            },
            LiveResetCredit {
                id: "bad-title".to_owned(),
                status: "available".to_owned(),
                expires_unix_seconds: None,
                expires_at: None,
                title: Some("unsafe\ntext".to_owned()),
            },
            LiveResetCredit {
                id: "bad-expiry".to_owned(),
                status: "available".to_owned(),
                expires_unix_seconds: None,
                expires_at: Some("malformed".to_owned()),
                title: None,
            },
        ] {
            assert!(validate_credit_inventory(vec![credit], 100).is_err());
        }
    }

    #[test]
    fn inventory_pages_are_deterministic_and_clamped() {
        assert_eq!(inventory_page(9, 0, 4), InventoryPage::new(0, 4, 9));
        assert_eq!(inventory_page(9, 4, 4), InventoryPage::new(4, 8, 9));
        assert_eq!(inventory_page(9, 8, 4), InventoryPage::new(8, 9, 9));
        assert_eq!(inventory_page(9, 99, 4), InventoryPage::new(8, 9, 9));
        assert_eq!(inventory_page(0, 0, 4), InventoryPage::new(0, 0, 0));
    }

    #[test]
    fn redeem_request_identity_is_bounded_and_control_character_free() {
        assert!(RedeemRequestId::new("redeem-1".to_owned()).is_ok());
        assert!(RedeemRequestId::new(String::new()).is_err());
        assert!(RedeemRequestId::new("bad\nvalue".to_owned()).is_err());
        assert!(RedeemRequestId::new("x".repeat(257)).is_err());
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
