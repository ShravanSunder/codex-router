use thiserror::Error;

/// Per-account choice to spend provider-reported ChatGPT usage credits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CreditUsagePolicy {
    /// Never route requests using usage credits.
    #[default]
    Disallow,
    /// Permit eligible exhausted Responses requests to use usage credits.
    Allow,
}

impl CreditUsagePolicy {
    /// Returns whether this policy opts into eligible credit-backed routing.
    #[must_use]
    pub const fn allows_credit_usage(self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// A provider-reported nonnegative decimal balance, retaining its original precision.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CreditBalance(String);

impl CreditBalance {
    /// Validates a nonnegative decimal without rounding or normalizing its spelling.
    pub fn new(value: impl Into<String>) -> Result<Self, CreditBalanceError> {
        let value = value.into();
        if !is_nonnegative_decimal(&value) {
            return Err(CreditBalanceError::InvalidDecimal);
        }

        Ok(Self(value))
    }

    /// Returns the exact provider decimal spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns whether the exact decimal has a nonzero digit.
    #[must_use]
    pub fn is_positive(&self) -> bool {
        self.0
            .bytes()
            .any(|byte| byte.is_ascii_digit() && byte != b'0')
    }
}

/// Invalid provider-reported credit balance.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CreditBalanceError {
    /// The balance is not an unsigned base-10 decimal with at least one integer digit.
    #[error("credit balance must be a nonnegative decimal")]
    InvalidDecimal,
}

/// Validated provider credit entitlement for one observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreditAvailability {
    /// The provider omitted or rejected credit authority.
    Unknown,
    /// The provider reports no available credit entitlement.
    Depleted,
    /// The provider reports credit entitlement, optionally withholding its balance.
    Available { balance: Option<CreditBalance> },
    /// The provider reports an unlimited credit entitlement.
    Unlimited,
}

impl CreditAvailability {
    /// Returns the stable storage tag, excluding an optional Available balance.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Depleted => "depleted",
            Self::Available { .. } => "available",
            Self::Unlimited => "unlimited",
        }
    }

    /// Reconstructs a validated state tag and its optional persisted balance.
    #[must_use]
    pub fn from_stored_parts(value: &str, balance: Option<CreditBalance>) -> Option<Self> {
        match (value, balance) {
            ("unknown", None) => Some(Self::Unknown),
            ("depleted", None) => Some(Self::Depleted),
            ("available", balance) => Some(Self::Available { balance }),
            ("unlimited", None) => Some(Self::Unlimited),
            _ => None,
        }
    }

    /// Returns whether this provider entitlement can back a credit-routing assessment.
    #[must_use]
    pub fn can_authorize_spending(&self) -> bool {
        match self {
            Self::Unknown | Self::Depleted => false,
            Self::Available { balance: None } | Self::Unlimited => true,
            Self::Available {
                balance: Some(balance),
            } => balance.is_positive(),
        }
    }
}

/// Provider spend-control authority for credit-backed routing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreditSpendControl {
    /// Provider spend-control authority was malformed or could not be classified.
    Unknown,
    /// Provider omitted or nullified optional spend-control details.
    Unreported,
    /// Provider did not report a spend-control rejection.
    Clear,
    /// Provider reports that its spend control has been reached.
    Reached,
}

impl CreditSpendControl {
    /// Returns the stable value stored for this state.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unreported => "unreported",
            Self::Clear => "clear",
            Self::Reached => "reached",
        }
    }

    /// Parses a stable stored state.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "unreported" => Some(Self::Unreported),
            "clear" => Some(Self::Clear),
            "reached" => Some(Self::Reached),
            _ => None,
        }
    }

    /// Returns whether provider spend-control evidence blocks credit-backed routing.
    #[must_use]
    pub const fn blocks_credit_usage(&self) -> bool {
        matches!(self, Self::Unknown | Self::Reached)
    }
}

/// Closed provider reason associated with a quota or usage-limit response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreditProviderLimitReason {
    /// Provider returned an unrecognized or malformed non-null reason.
    Unknown,
    /// Ordinary included quota was exhausted; available usage credits may still be used.
    RateLimitReached,
    /// Workspace owner credits were depleted.
    WorkspaceOwnerCreditsDepleted,
    /// Workspace member credits were depleted.
    WorkspaceMemberCreditsDepleted,
    /// Workspace owner usage limit was reached.
    WorkspaceOwnerUsageLimitReached,
    /// Workspace member usage limit was reached.
    WorkspaceMemberUsageLimitReached,
}

impl CreditProviderLimitReason {
    /// Returns the exact provider or storage name for this reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::RateLimitReached => "rate_limit_reached",
            Self::WorkspaceOwnerCreditsDepleted => "workspace_owner_credits_depleted",
            Self::WorkspaceMemberCreditsDepleted => "workspace_member_credits_depleted",
            Self::WorkspaceOwnerUsageLimitReached => "workspace_owner_usage_limit_reached",
            Self::WorkspaceMemberUsageLimitReached => "workspace_member_usage_limit_reached",
        }
    }

    /// Parses one provider reason or its stable storage value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "rate_limit_reached" => Some(Self::RateLimitReached),
            "workspace_owner_credits_depleted" => Some(Self::WorkspaceOwnerCreditsDepleted),
            "workspace_member_credits_depleted" => Some(Self::WorkspaceMemberCreditsDepleted),
            "workspace_owner_usage_limit_reached" => Some(Self::WorkspaceOwnerUsageLimitReached),
            "workspace_member_usage_limit_reached" => Some(Self::WorkspaceMemberUsageLimitReached),
            _ => None,
        }
    }

    /// Returns whether this provider reason rejects usage-credit routing.
    #[must_use]
    pub const fn blocks_credit_usage(self) -> bool {
        !matches!(self, Self::RateLimitReached)
    }
}

/// Validated credit authority and provider-side restrictions from one usage response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreditProviderObservation {
    availability: CreditAvailability,
    spend_control: CreditSpendControl,
    limit_reason: Option<CreditProviderLimitReason>,
}

impl CreditProviderObservation {
    /// Builds one provider observation from independently validated response fields.
    #[must_use]
    pub fn new(
        availability: CreditAvailability,
        spend_control: CreditSpendControl,
        limit_reason: Option<CreditProviderLimitReason>,
    ) -> Self {
        Self {
            availability,
            spend_control,
            limit_reason,
        }
    }

    /// Returns the fail-closed value for a response without recognized credit facts.
    #[must_use]
    pub fn missing() -> Self {
        Self::new(
            CreditAvailability::Unknown,
            CreditSpendControl::Unreported,
            None,
        )
    }

    /// Returns provider-reported credit entitlement.
    #[must_use]
    pub const fn availability(&self) -> &CreditAvailability {
        &self.availability
    }

    /// Returns provider spend-control state.
    #[must_use]
    pub const fn spend_control(&self) -> &CreditSpendControl {
        &self.spend_control
    }

    /// Returns the optional top-level provider limit reason.
    #[must_use]
    pub const fn limit_reason(&self) -> Option<CreditProviderLimitReason> {
        self.limit_reason
    }

    /// Returns whether provider facts alone permit usage-credit routing.
    #[must_use]
    pub fn authorizes_credit_usage(&self) -> bool {
        self.availability.can_authorize_spending()
            && !self.spend_control.blocks_credit_usage()
            && !self
                .limit_reason
                .is_some_and(CreditProviderLimitReason::blocks_credit_usage)
    }
}

fn is_nonnegative_decimal(value: &str) -> bool {
    let Some((integer_digits, fractional_digits)) = value.split_once('.') else {
        return !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    };

    !integer_digits.is_empty()
        && !fractional_digits.is_empty()
        && integer_digits.bytes().all(|byte| byte.is_ascii_digit())
        && fractional_digits.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::CreditAvailability;
    use super::CreditBalance;
    use super::CreditUsagePolicy;

    #[test]
    fn credit_balance_preserves_decimal_precision_and_distinguishes_zero() {
        let balance = CreditBalance::new("0003.1400")
            .expect("provider decimal balance should validate without rounding");
        let zero = CreditBalance::new("0.000")
            .expect("zero decimal balance should remain a valid observation");

        assert_eq!(balance.as_str(), "0003.1400");
        assert!(balance.is_positive());
        assert_eq!(zero.as_str(), "0.000");
        assert!(!zero.is_positive());
    }

    #[test]
    fn credit_balance_rejects_malformed_or_negative_decimal_values() {
        for invalid_balance in ["", " ", "-1", "+1", ".5", "1.", "1e3", "1,000", "1.2.3"] {
            assert!(
                CreditBalance::new(invalid_balance).is_err(),
                "invalid provider balance {invalid_balance:?} should be rejected"
            );
        }
    }

    #[test]
    fn credit_availability_authorizes_reported_entitlement_but_never_zero_or_unknown() {
        let positive_balance = CreditBalance::new("0.01").expect("positive balance");
        let zero_balance = CreditBalance::new("0").expect("zero balance");

        assert!(CreditAvailability::Unlimited.can_authorize_spending());
        assert!(
            CreditAvailability::Available {
                balance: Some(positive_balance),
            }
            .can_authorize_spending()
        );
        assert!(
            CreditAvailability::Available { balance: None }.can_authorize_spending(),
            "has_credits authority remains valid when the provider withholds the balance"
        );
        for unavailable in [
            CreditAvailability::Unknown,
            CreditAvailability::Depleted,
            CreditAvailability::Available {
                balance: Some(zero_balance),
            },
        ] {
            assert!(
                !unavailable.can_authorize_spending(),
                "{unavailable:?} must not authorize credit routing"
            );
        }
    }

    #[test]
    fn credit_usage_policy_defaults_to_disallow_and_supports_explicit_opt_in() {
        assert_eq!(CreditUsagePolicy::default(), CreditUsagePolicy::Disallow);
        assert!(!CreditUsagePolicy::Disallow.allows_credit_usage());
        assert!(CreditUsagePolicy::Allow.allows_credit_usage());
    }
}
