use codex_router_core::ids::AccountId;

use crate::quota_reset::reset_credit_policy::ActiveCredentialGeneration;
use crate::quota_reset::reset_credit_policy::AttemptGeneration;
use crate::quota_reset::reset_credit_policy::ConsumePortResult;
use crate::quota_reset::reset_credit_policy::CreditInventoryPortResult;
use crate::quota_reset::reset_credit_policy::LiveUsagePortResult;
use crate::quota_reset::reset_credit_policy::OperationGeneration;
use crate::quota_reset::reset_credit_policy::RenderSafeFailure;

/// Correlation identity repeated by every request and completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) struct OperationCorrelation {
    account_id: AccountId,
    active_credential_generation: ActiveCredentialGeneration,
    attempt_generation: AttemptGeneration,
    operation_generation: OperationGeneration,
}

impl OperationCorrelation {
    pub(in crate::quota_reset) const fn new(
        account_id: AccountId,
        active_credential_generation: ActiveCredentialGeneration,
        attempt_generation: AttemptGeneration,
        operation_generation: OperationGeneration,
    ) -> Self {
        Self {
            account_id,
            active_credential_generation,
            attempt_generation,
            operation_generation,
        }
    }

    pub(in crate::quota_reset) const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub(in crate::quota_reset) const fn active_credential_generation(
        &self,
    ) -> ActiveCredentialGeneration {
        self.active_credential_generation
    }

    pub(in crate::quota_reset) const fn attempt_generation(&self) -> AttemptGeneration {
        self.attempt_generation
    }
}

/// Typed workflow effect request whose variant determines its operation kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) enum CorrelatedRequest {
    InspectionLiveUsage(OperationCorrelation),
    InspectionCreditInventory(OperationCorrelation),
    RevalidationLiveUsage(OperationCorrelation),
    RevalidationCreditInventory(OperationCorrelation),
    ConsumeCredit(OperationCorrelation),
}

impl CorrelatedRequest {
    pub(in crate::quota_reset) const fn inspection_live_usage(
        correlation: OperationCorrelation,
    ) -> Self {
        Self::InspectionLiveUsage(correlation)
    }

    pub(in crate::quota_reset) const fn inspection_credit_inventory(
        correlation: OperationCorrelation,
    ) -> Self {
        Self::InspectionCreditInventory(correlation)
    }

    pub(in crate::quota_reset) const fn revalidation_live_usage(
        correlation: OperationCorrelation,
    ) -> Self {
        Self::RevalidationLiveUsage(correlation)
    }

    pub(in crate::quota_reset) const fn revalidation_credit_inventory(
        correlation: OperationCorrelation,
    ) -> Self {
        Self::RevalidationCreditInventory(correlation)
    }

    pub(in crate::quota_reset) const fn consume_credit(correlation: OperationCorrelation) -> Self {
        Self::ConsumeCredit(correlation)
    }

    pub(in crate::quota_reset) const fn correlation(&self) -> &OperationCorrelation {
        match self {
            Self::InspectionLiveUsage(correlation)
            | Self::InspectionCreditInventory(correlation)
            | Self::RevalidationLiveUsage(correlation)
            | Self::RevalidationCreditInventory(correlation)
            | Self::ConsumeCredit(correlation) => correlation,
        }
    }
}

/// Typed workflow completion whose variant constrains its terminal category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) enum CorrelatedOutcome {
    InspectionLiveUsage {
        correlation: OperationCorrelation,
        terminal: LiveUsagePortResult,
    },
    InspectionCreditInventory {
        correlation: OperationCorrelation,
        terminal: CreditInventoryPortResult,
    },
    RevalidationLiveUsage {
        correlation: OperationCorrelation,
        terminal: LiveUsagePortResult,
    },
    RevalidationCreditInventory {
        correlation: OperationCorrelation,
        terminal: CreditInventoryPortResult,
    },
    ConsumeCredit {
        correlation: OperationCorrelation,
        terminal: ConsumePortResult,
    },
}

impl CorrelatedOutcome {
    pub(in crate::quota_reset) const fn inspection_live_usage(
        correlation: OperationCorrelation,
        terminal: LiveUsagePortResult,
    ) -> Self {
        Self::InspectionLiveUsage {
            correlation,
            terminal,
        }
    }

    pub(in crate::quota_reset) const fn inspection_credit_inventory(
        correlation: OperationCorrelation,
        terminal: CreditInventoryPortResult,
    ) -> Self {
        Self::InspectionCreditInventory {
            correlation,
            terminal,
        }
    }

    pub(in crate::quota_reset) const fn revalidation_live_usage(
        correlation: OperationCorrelation,
        terminal: LiveUsagePortResult,
    ) -> Self {
        Self::RevalidationLiveUsage {
            correlation,
            terminal,
        }
    }

    pub(in crate::quota_reset) const fn revalidation_credit_inventory(
        correlation: OperationCorrelation,
        terminal: CreditInventoryPortResult,
    ) -> Self {
        Self::RevalidationCreditInventory {
            correlation,
            terminal,
        }
    }

    pub(in crate::quota_reset) const fn consume_credit(
        correlation: OperationCorrelation,
        terminal: ConsumePortResult,
    ) -> Self {
        Self::ConsumeCredit {
            correlation,
            terminal,
        }
    }
}

/// Render-safe workflow phase without authority-bearing values.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum WorkflowPhase {
    #[default]
    Browse,
    Inspecting,
    Inspected,
    Confirming,
    Revalidating,
    Committing,
    Result,
}

/// Explicit provenance for values that may be shown in reset detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenderValueProvenance {
    CurrentLive,
    PreviousLiveRefreshing,
}

/// Render-safe semantic activity state for one provider operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationActivity<TValue> {
    NotStarted,
    Loading,
    Refreshing {
        previous: Option<TValue>,
    },
    Succeeded(TValue),
    Failed {
        failure: RenderSafeFailure,
        previous: Option<TValue>,
    },
    Cancelled,
    RequestDispatchedAwaitingOutcome,
}
