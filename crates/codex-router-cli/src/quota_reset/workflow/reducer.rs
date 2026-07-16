use crate::quota_reset::domain::ConsumePortResult;
use crate::quota_reset::domain::CreditInventoryPortResult;
use crate::quota_reset::domain::LiveUsagePortResult;
use crate::quota_reset::domain::LiveWeeklyUsage;
use crate::quota_reset::domain::OperationGeneration;
use crate::quota_reset::domain::RenderSafeFailure;

use super::contracts::CorrelatedOutcome;
use super::contracts::CorrelatedRequest;
use super::contracts::OperationActivity;
use super::contracts::OperationCorrelation;
use super::contracts::RenderValueProvenance;
use super::contracts::WorkflowPhase;
use super::model::AuthorityObservation;
use super::model::ConfirmationSelection;
use super::model::InspectionStart;
use super::model::OperationSuccess;
use super::model::ResetWorkflow;
use super::model::WorkflowActivities;
use super::model::WorkflowIntent;
use super::model::WorkflowResult;

impl ResetWorkflow {
    pub(in crate::quota_reset) const fn phase(&self) -> WorkflowPhase {
        self.phase
    }

    pub(in crate::quota_reset) const fn confirmation_selection(&self) -> ConfirmationSelection {
        self.confirmation_selection
    }

    pub(in crate::quota_reset) const fn result(&self) -> Option<&WorkflowResult> {
        self.result.as_ref()
    }

    pub(in crate::quota_reset) const fn activities(&self) -> &WorkflowActivities {
        &self.activities
    }

    pub(in crate::quota_reset) fn live_usage_observation(
        &self,
    ) -> Option<(LiveWeeklyUsage, RenderValueProvenance)> {
        self.live_usage
            .as_ref()
            .map(|observation| (observation.value, observation.provenance))
    }

    pub(in crate::quota_reset) fn inventory_observation(
        &self,
    ) -> Option<(
        &crate::quota_reset::domain::ValidatedCreditInventory,
        RenderValueProvenance,
    )> {
        self.inventory
            .as_ref()
            .map(|observation| (&observation.value, observation.provenance))
    }

    pub(in crate::quota_reset) const fn authority_failure(&self) -> Option<RenderSafeFailure> {
        self.authority_failure
    }

    pub(in crate::quota_reset) fn consume_correlation(&self) -> Option<OperationCorrelation> {
        self.consume.clone()
    }

    pub(in crate::quota_reset) fn yes_enabled(&self) -> bool {
        let current_authority_operations_succeeded = match self.phase {
            WorkflowPhase::Inspected | WorkflowPhase::Confirming => {
                activity_succeeded(&self.activities.inspection_live_usage)
                    && activity_succeeded(&self.activities.inspection_credit_inventory)
            }
            WorkflowPhase::Revalidating => {
                activity_succeeded(&self.activities.revalidation_live_usage)
                    && activity_succeeded(&self.activities.revalidation_credit_inventory)
            }
            WorkflowPhase::Browse
            | WorkflowPhase::Inspecting
            | WorkflowPhase::Committing
            | WorkflowPhase::Result => false,
        };
        current_authority_operations_succeeded
            && self.authority_failure.is_none()
            && self.live_usage.as_ref().is_some_and(|usage| {
                usage.provenance == RenderValueProvenance::CurrentLive
                    && usage.value.remaining_percent() < 1
            })
            && self.inventory.as_ref().is_some_and(|inventory| {
                inventory.provenance == RenderValueProvenance::CurrentLive
                    && inventory.value.earliest_usable_credit_id().is_some()
            })
    }

    pub(in crate::quota_reset) fn reduce(
        &mut self,
        intent: WorkflowIntent,
    ) -> Vec<CorrelatedRequest> {
        match intent {
            WorkflowIntent::BeginInspection(start) => self.begin_inspection(start),
            WorkflowIntent::OperationCompleted(outcome) => {
                self.apply_operation_outcome(outcome);
                Vec::new()
            }
            WorkflowIntent::OpenConfirmation if self.phase == WorkflowPhase::Inspected => {
                self.phase = WorkflowPhase::Confirming;
                self.confirmation_selection = ConfirmationSelection::No;
                self.confirmed_credit = self
                    .inventory
                    .as_ref()
                    .and_then(|inventory| inventory.value.earliest_usable_identity());
                Vec::new()
            }
            WorkflowIntent::SelectNo if self.phase == WorkflowPhase::Confirming => {
                self.confirmation_selection = ConfirmationSelection::No;
                Vec::new()
            }
            WorkflowIntent::SelectYes
                if self.phase == WorkflowPhase::Confirming && self.yes_enabled() =>
            {
                self.confirmation_selection = ConfirmationSelection::Yes;
                Vec::new()
            }
            WorkflowIntent::Confirm {
                live_usage_operation_generation,
                credit_inventory_operation_generation,
            } => self.confirm(
                live_usage_operation_generation,
                credit_inventory_operation_generation,
            ),
            WorkflowIntent::CommitAuthorized {
                consume_operation_generation,
            } => self.commit_authorized(consume_operation_generation),
            WorkflowIntent::RevalidationRefused {
                live_usage_correlation,
                credit_inventory_correlation,
                failure,
            } => {
                self.revalidation_refused(
                    live_usage_correlation,
                    credit_inventory_correlation,
                    failure,
                );
                Vec::new()
            }
            WorkflowIntent::AuthorityLost(failure) => {
                self.authority_failure = Some(failure);
                self.confirmation_selection = ConfirmationSelection::No;
                Vec::new()
            }
            WorkflowIntent::PinnedTargetInvalidated(failure)
                if self.phase != WorkflowPhase::Committing =>
            {
                self.cancel_precommit();
                self.authority_failure = Some(failure);
                Vec::new()
            }
            WorkflowIntent::Cancel if self.phase != WorkflowPhase::Committing => {
                self.cancel_precommit();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn begin_inspection(&mut self, start: InspectionStart) -> Vec<CorrelatedRequest> {
        if self.phase != WorkflowPhase::Browse {
            return Vec::new();
        }
        if let Some(previous) = &mut self.live_usage
            && previous.provenance == RenderValueProvenance::CurrentLive
        {
            previous.provenance = RenderValueProvenance::PreviousLiveRefreshing;
        }
        if let Some(previous) = &mut self.inventory
            && previous.provenance == RenderValueProvenance::CurrentLive
        {
            previous.provenance = RenderValueProvenance::PreviousLiveRefreshing;
        }
        self.phase = WorkflowPhase::Inspecting;
        self.inspection = Some(start.clone());
        self.authority_failure = None;
        self.confirmation_selection = ConfirmationSelection::No;
        self.activities.inspection_live_usage = OperationActivity::Loading;
        self.activities.inspection_credit_inventory = OperationActivity::Loading;
        vec![
            CorrelatedRequest::inspection_live_usage(start.live_usage_correlation()),
            CorrelatedRequest::inspection_credit_inventory(start.credit_inventory_correlation()),
        ]
    }

    fn confirm(
        &mut self,
        live_usage_operation_generation: OperationGeneration,
        credit_inventory_operation_generation: OperationGeneration,
    ) -> Vec<CorrelatedRequest> {
        if self.phase != WorkflowPhase::Confirming {
            return Vec::new();
        }
        if self.confirmation_selection == ConfirmationSelection::No {
            self.cancel_precommit();
            return Vec::new();
        }
        if !self.yes_enabled() {
            self.confirmation_selection = ConfirmationSelection::No;
            return Vec::new();
        }
        let Some(inspection) = &self.inspection else {
            return Vec::new();
        };
        let basis = inspection.live_usage_correlation();
        let usage = OperationCorrelation::new(
            basis.account_id().clone(),
            basis.active_credential_generation(),
            basis.attempt_generation(),
            live_usage_operation_generation,
        );
        let inventory = OperationCorrelation::new(
            basis.account_id().clone(),
            basis.active_credential_generation(),
            basis.attempt_generation(),
            credit_inventory_operation_generation,
        );
        self.phase = WorkflowPhase::Revalidating;
        self.revalidation_usage = Some(usage.clone());
        self.revalidation_inventory = Some(inventory.clone());
        let previous_usage = self
            .live_usage
            .as_ref()
            .map(|observation| OperationSuccess::LiveUsage(observation.value));
        let previous_inventory =
            self.inventory
                .as_ref()
                .map(|observation| OperationSuccess::CreditInventory {
                    credit_count: observation.value.len(),
                    usable_credit_count: observation.value.usable_credit_count(),
                });
        if let Some(observation) = self.live_usage.as_mut() {
            observation.provenance = RenderValueProvenance::PreviousLiveRefreshing;
        }
        if let Some(observation) = self.inventory.as_mut() {
            observation.provenance = RenderValueProvenance::PreviousLiveRefreshing;
        }
        self.activities.revalidation_live_usage = OperationActivity::Refreshing {
            previous: previous_usage,
        };
        self.activities.revalidation_credit_inventory = OperationActivity::Refreshing {
            previous: previous_inventory,
        };
        vec![
            CorrelatedRequest::revalidation_live_usage(usage),
            CorrelatedRequest::revalidation_credit_inventory(inventory),
        ]
    }

    fn commit_authorized(
        &mut self,
        consume_operation_generation: OperationGeneration,
    ) -> Vec<CorrelatedRequest> {
        if self.phase != WorkflowPhase::Revalidating
            || !activity_succeeded(&self.activities.revalidation_live_usage)
            || !activity_succeeded(&self.activities.revalidation_credit_inventory)
            || !self.yes_enabled()
        {
            return Vec::new();
        }
        let Some(basis) = self.revalidation_usage.as_ref() else {
            return Vec::new();
        };
        let correlation = OperationCorrelation::new(
            basis.account_id().clone(),
            basis.active_credential_generation(),
            basis.attempt_generation(),
            consume_operation_generation,
        );
        self.phase = WorkflowPhase::Committing;
        self.consume = Some(correlation.clone());
        self.activities.consume_credit = OperationActivity::RequestDispatchedAwaitingOutcome;
        vec![CorrelatedRequest::consume_credit(correlation)]
    }

    fn revalidation_refused(
        &mut self,
        live_usage_correlation: OperationCorrelation,
        credit_inventory_correlation: OperationCorrelation,
        failure: RenderSafeFailure,
    ) {
        if self.phase != WorkflowPhase::Revalidating
            || self.revalidation_usage.as_ref() != Some(&live_usage_correlation)
            || self.revalidation_inventory.as_ref() != Some(&credit_inventory_correlation)
        {
            return;
        }
        self.phase = WorkflowPhase::Result;
        self.confirmation_selection = ConfirmationSelection::No;
        self.authority_failure = Some(failure);
        self.result = Some(WorkflowResult::Refused(failure));
    }

    fn apply_operation_outcome(&mut self, outcome: CorrelatedOutcome) {
        match outcome {
            CorrelatedOutcome::InspectionLiveUsage {
                correlation,
                terminal,
            } if self.phase == WorkflowPhase::Inspecting
                && self
                    .inspection
                    .as_ref()
                    .is_some_and(|expected| expected.live_usage == correlation)
                && !activity_terminal(&self.activities.inspection_live_usage) =>
            {
                self.apply_usage_terminal(terminal, true);
                self.finish_inspection_if_terminal();
            }
            CorrelatedOutcome::InspectionCreditInventory {
                correlation,
                terminal,
            } if self.phase == WorkflowPhase::Inspecting
                && self
                    .inspection
                    .as_ref()
                    .is_some_and(|expected| expected.credit_inventory == correlation)
                && !activity_terminal(&self.activities.inspection_credit_inventory) =>
            {
                self.apply_inventory_terminal(terminal, true);
                self.finish_inspection_if_terminal();
            }
            CorrelatedOutcome::RevalidationLiveUsage {
                correlation,
                terminal,
            } if self.phase == WorkflowPhase::Revalidating
                && self.revalidation_usage.as_ref() == Some(&correlation)
                && !activity_terminal(&self.activities.revalidation_live_usage) =>
            {
                self.apply_usage_terminal(terminal, false);
            }
            CorrelatedOutcome::RevalidationCreditInventory {
                correlation,
                terminal,
            } if self.phase == WorkflowPhase::Revalidating
                && self.revalidation_inventory.as_ref() == Some(&correlation)
                && !activity_terminal(&self.activities.revalidation_credit_inventory) =>
            {
                self.apply_inventory_terminal(terminal, false);
            }
            CorrelatedOutcome::ConsumeCredit {
                correlation,
                terminal,
            } if self.phase == WorkflowPhase::Committing
                && self.consume.as_ref() == Some(&correlation) =>
            {
                self.result = Some(match terminal {
                    ConsumePortResult::Known(outcome) => {
                        self.activities.consume_credit = OperationActivity::Succeeded(
                            OperationSuccess::Consume(outcome.clone()),
                        );
                        WorkflowResult::Known(outcome)
                    }
                    ConsumePortResult::OutcomeUnknown(reason) => {
                        self.activities.consume_credit = OperationActivity::Failed {
                            failure: match reason {
                                crate::quota_reset::domain::ConsumeUnknownReason::Transport => {
                                    RenderSafeFailure::Transport
                                }
                                crate::quota_reset::domain::ConsumeUnknownReason::TimedOut => {
                                    RenderSafeFailure::TimedOut
                                }
                                crate::quota_reset::domain::ConsumeUnknownReason::ProviderStatus => {
                                    RenderSafeFailure::ProviderStatus
                                }
                                crate::quota_reset::domain::ConsumeUnknownReason::InvalidResponse => {
                                    RenderSafeFailure::InvalidResponse
                                }
                            },
                            previous: None,
                        };
                        WorkflowResult::OutcomeUnknown(reason)
                    }
                });
                self.phase = WorkflowPhase::Result;
            }
            _ => {}
        }
    }

    fn apply_usage_terminal(&mut self, terminal: LiveUsagePortResult, inspection: bool) {
        let activity = if inspection {
            &mut self.activities.inspection_live_usage
        } else {
            &mut self.activities.revalidation_live_usage
        };
        match terminal {
            LiveUsagePortResult::Known(usage) => {
                self.live_usage = Some(AuthorityObservation {
                    value: usage,
                    provenance: RenderValueProvenance::CurrentLive,
                });
                *activity = OperationActivity::Succeeded(OperationSuccess::LiveUsage(usage));
            }
            LiveUsagePortResult::Failed(failure) => {
                *activity = OperationActivity::Failed {
                    failure,
                    previous: None,
                };
                self.confirmation_selection = ConfirmationSelection::No;
            }
        }
    }

    fn apply_inventory_terminal(&mut self, terminal: CreditInventoryPortResult, inspection: bool) {
        let activity = if inspection {
            &mut self.activities.inspection_credit_inventory
        } else {
            &mut self.activities.revalidation_credit_inventory
        };
        match terminal {
            CreditInventoryPortResult::Validated(inventory) => {
                let selected_credit_changed = !inspection
                    && self.confirmed_credit.is_some()
                    && inventory.earliest_usable_identity() != self.confirmed_credit;
                let success = OperationSuccess::CreditInventory {
                    credit_count: inventory.len(),
                    usable_credit_count: inventory.usable_credit_count(),
                };
                self.inventory = Some(AuthorityObservation {
                    value: inventory,
                    provenance: RenderValueProvenance::CurrentLive,
                });
                *activity = OperationActivity::Succeeded(success);
                if selected_credit_changed {
                    self.authority_failure = Some(RenderSafeFailure::SelectedCreditChanged);
                    self.confirmation_selection = ConfirmationSelection::No;
                }
            }
            CreditInventoryPortResult::Failed(failure) => {
                *activity = OperationActivity::Failed {
                    failure,
                    previous: None,
                };
                self.confirmation_selection = ConfirmationSelection::No;
            }
        }
    }

    fn finish_inspection_if_terminal(&mut self) {
        if activity_terminal(&self.activities.inspection_live_usage)
            && activity_terminal(&self.activities.inspection_credit_inventory)
        {
            self.phase = WorkflowPhase::Inspected;
        }
    }

    fn cancel_precommit(&mut self) {
        cancel_nonterminal_activity(&mut self.activities.inspection_live_usage);
        cancel_nonterminal_activity(&mut self.activities.inspection_credit_inventory);
        cancel_nonterminal_activity(&mut self.activities.revalidation_live_usage);
        cancel_nonterminal_activity(&mut self.activities.revalidation_credit_inventory);
        self.phase = WorkflowPhase::Browse;
        self.confirmation_selection = ConfirmationSelection::No;
        self.inspection = None;
        self.revalidation_usage = None;
        self.revalidation_inventory = None;
        self.authority_failure = None;
        self.confirmed_credit = None;
        self.result = None;
    }
}

fn cancel_nonterminal_activity(activity: &mut OperationActivity<OperationSuccess>) {
    if matches!(
        activity,
        OperationActivity::Loading | OperationActivity::Refreshing { .. }
    ) {
        *activity = OperationActivity::Cancelled;
    }
}

fn activity_terminal(activity: &OperationActivity<OperationSuccess>) -> bool {
    matches!(
        activity,
        OperationActivity::Succeeded(_)
            | OperationActivity::Failed { .. }
            | OperationActivity::Cancelled
    )
}

fn activity_succeeded(activity: &OperationActivity<OperationSuccess>) -> bool {
    matches!(activity, OperationActivity::Succeeded(_))
}
