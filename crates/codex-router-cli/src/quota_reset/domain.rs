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
}

/// Unique identity for one operation within an attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::quota_reset) struct OperationGeneration(u64);

impl OperationGeneration {
    pub(in crate::quota_reset) const fn new(value: u64) -> Self {
        Self(value)
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

    pub(crate) fn earliest_usable_credit_id(&self) -> Option<&str> {
        self.earliest_usable_index
            .and_then(|index| self.credits.get(index))
            .map(|credit| credit.identity.0.as_str())
    }

    pub(crate) const fn usable_credit_count(&self) -> usize {
        self.usable_credit_count
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

    pub(in crate::quota_reset) fn display_projection(&self) -> Vec<ResetCreditDisplayProjection> {
        self.credits
            .iter()
            .enumerate()
            .map(|(index, credit)| ResetCreditDisplayProjection {
                id_hint: safe_credit_id_hint(&credit.identity.0),
                status: match credit.status {
                    ValidatedCreditStatus::Available => ResetCreditDisplayStatus::Available,
                    ValidatedCreditStatus::Redeeming => ResetCreditDisplayStatus::Redeeming,
                    ValidatedCreditStatus::Redeemed => ResetCreditDisplayStatus::Redeemed,
                },
                title: credit.title.clone(),
                expires_unix_seconds: credit.expires_unix_seconds,
                earliest_usable: self.earliest_usable_index == Some(index),
            })
            .collect()
    }
}

/// Redacted inventory entry safe to copy into the presentation protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) struct ResetCreditDisplayProjection {
    pub(in crate::quota_reset) id_hint: String,
    pub(in crate::quota_reset) status: ResetCreditDisplayStatus,
    pub(in crate::quota_reset) title: Option<String>,
    pub(in crate::quota_reset) expires_unix_seconds: Option<i64>,
    pub(in crate::quota_reset) earliest_usable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) enum ResetCreditDisplayStatus {
    Available,
    Redeeming,
    Redeemed,
}

fn safe_credit_id_hint(identity: &str) -> String {
    let suffix = identity.chars().rev().take(4).collect::<Vec<_>>();
    if identity.chars().count() <= suffix.len() {
        return "hidden".to_owned();
    }
    format!("…{}", suffix.into_iter().rev().collect::<String>())
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

#[cfg(test)]
mod tests;
