use crate::quota_reset::reset_credit_policy::ConsumeUnknownReason;
use crate::quota_reset::reset_credit_policy::KnownConsumeOutcome;
use crate::quota_reset::reset_credit_policy::LiveWeeklyUsage;
use crate::quota_reset::reset_credit_policy::OperationGeneration;
use crate::quota_reset::reset_credit_policy::RenderSafeFailure;
use crate::quota_reset::reset_credit_policy::ResetCreditIdentity;
use crate::quota_reset::reset_credit_policy::ValidatedCreditInventory;

use super::correlated_effect_contracts::CorrelatedOutcome;
use super::correlated_effect_contracts::OperationActivity;
use super::correlated_effect_contracts::OperationCorrelation;
use super::correlated_effect_contracts::RenderValueProvenance;
use super::correlated_effect_contracts::WorkflowPhase;

/// Correlated identities required to start independent inspection reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) struct InspectionStart {
    pub(super) live_usage: OperationCorrelation,
    pub(super) credit_inventory: OperationCorrelation,
}

impl InspectionStart {
    pub(in crate::quota_reset) fn new(
        account_id: AccountId,
        active_credential_generation: ActiveCredentialGeneration,
        attempt_generation: AttemptGeneration,
        live_usage_operation_generation: OperationGeneration,
        credit_inventory_operation_generation: OperationGeneration,
    ) -> Self {
        Self {
            live_usage: OperationCorrelation::new(
                account_id.clone(),
                active_credential_generation,
                attempt_generation,
                live_usage_operation_generation,
            ),
            credit_inventory: OperationCorrelation::new(
                account_id,
                active_credential_generation,
                attempt_generation,
                credit_inventory_operation_generation,
            ),
        }
    }

    pub(in crate::quota_reset) fn live_usage_correlation(&self) -> OperationCorrelation {
        self.live_usage.clone()
    }

    pub(in crate::quota_reset) fn credit_inventory_correlation(&self) -> OperationCorrelation {
        self.credit_inventory.clone()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ConfirmationSelection {
    #[default]
    No,
    Yes,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowResult {
    Known(KnownConsumeOutcome),
    OutcomeUnknown(ConsumeUnknownReason),
    Refused(RenderSafeFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationSuccess {
    LiveUsage(LiveWeeklyUsage),
    CreditInventory {
        credit_count: usize,
        usable_credit_count: usize,
    },
    Consume(KnownConsumeOutcome),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthorityObservation<TValue> {
    pub(crate) value: TValue,
    pub(crate) provenance: RenderValueProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) enum WorkflowIntent {
    BeginInspection(InspectionStart),
    OperationCompleted(CorrelatedOutcome),
    OpenConfirmation,
    SelectNo,
    SelectYes,
    Confirm {
        live_usage_operation_generation: OperationGeneration,
        credit_inventory_operation_generation: OperationGeneration,
    },
    CommitAuthorized {
        consume_operation_generation: OperationGeneration,
    },
    RevalidationRefused {
        live_usage_correlation: OperationCorrelation,
        credit_inventory_correlation: OperationCorrelation,
        failure: RenderSafeFailure,
    },
    AuthorityLost(RenderSafeFailure),
    PinnedTargetInvalidated(RenderSafeFailure),
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowActivities {
    pub(crate) inspection_live_usage: OperationActivity<OperationSuccess>,
    pub(crate) inspection_credit_inventory: OperationActivity<OperationSuccess>,
    pub(crate) revalidation_live_usage: OperationActivity<OperationSuccess>,
    pub(crate) revalidation_credit_inventory: OperationActivity<OperationSuccess>,
    pub(crate) consume_credit: OperationActivity<OperationSuccess>,
}

impl Default for WorkflowActivities {
    fn default() -> Self {
        Self {
            inspection_live_usage: OperationActivity::NotStarted,
            inspection_credit_inventory: OperationActivity::NotStarted,
            revalidation_live_usage: OperationActivity::NotStarted,
            revalidation_credit_inventory: OperationActivity::NotStarted,
            consume_credit: OperationActivity::NotStarted,
        }
    }
}

/// Pure correlated reset workflow reducer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) struct ResetWorkflow {
    pub(super) phase: WorkflowPhase,
    pub(super) inspection: Option<InspectionStart>,
    pub(super) revalidation_usage: Option<OperationCorrelation>,
    pub(super) revalidation_inventory: Option<OperationCorrelation>,
    pub(super) consume: Option<OperationCorrelation>,
    pub(super) confirmation_selection: ConfirmationSelection,
    pub(super) authority_failure: Option<RenderSafeFailure>,
    pub(super) live_usage: Option<AuthorityObservation<LiveWeeklyUsage>>,
    pub(super) inventory: Option<AuthorityObservation<ValidatedCreditInventory>>,
    pub(super) confirmed_credit: Option<ResetCreditIdentity>,
    pub(super) activities: WorkflowActivities,
    pub(super) result: Option<WorkflowResult>,
}

impl Default for ResetWorkflow {
    fn default() -> Self {
        Self {
            phase: WorkflowPhase::Browse,
            inspection: None,
            revalidation_usage: None,
            revalidation_inventory: None,
            consume: None,
            confirmation_selection: ConfirmationSelection::No,
            authority_failure: None,
            live_usage: None,
            inventory: None,
            confirmed_credit: None,
            activities: WorkflowActivities::default(),
            result: None,
        }
    }
}
use codex_router_core::ids::AccountId;

use crate::quota_reset::reset_credit_policy::ActiveCredentialGeneration;
use crate::quota_reset::reset_credit_policy::AttemptGeneration;
