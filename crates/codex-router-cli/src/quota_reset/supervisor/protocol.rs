//! Authority-free command-session ports shared with presentation.

use codex_router_core::ids::AccountId;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::quota_reset::domain::ResetCreditDisplayProjection;
use crate::quota_reset::domain::ResetCreditDisplayStatus;
use crate::quota_reset::workflow::ConfirmationSelection;
use crate::quota_reset::workflow::RenderValueProvenance;
use crate::quota_reset::workflow::ResetWorkflow;
use crate::quota_reset::workflow::WorkflowActivities;
use crate::quota_reset::workflow::WorkflowPhase;
use crate::quota_reset::workflow::WorkflowResult;

/// Presentation-originated commands. Provider completions cannot enter this port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResetSessionIntent {
    BeginInspection {
        account_id: AccountId,
        active_credential_generation: u64,
        now_unix_seconds: u64,
    },
    OpenConfirmation,
    SelectNo,
    SelectYes,
    Confirm {
        now_unix_seconds: u64,
    },
    Cancel,
    DismissResult,
    PinnedTargetInvalidated {
        account_id: AccountId,
        active_credential_generation: u64,
        reason: PinnedTargetInvalidationReason,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PinnedTargetInvalidationReason {
    AccountRemoved,
    AccountDisabled,
    CredentialGenerationChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PinnedResetTarget {
    pub(crate) account_id: AccountId,
    pub(crate) active_credential_generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResetValueProvenance {
    CurrentLive,
    PreviousLiveRefreshing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResetCreditDisplayStatusDto {
    Available,
    Redeeming,
    Redeemed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResetCreditDisplayRecord {
    pub(crate) id_hint: String,
    pub(crate) status: ResetCreditDisplayStatusDto,
    pub(crate) title: Option<String>,
    pub(crate) expires_unix_seconds: Option<i64>,
    pub(crate) earliest_usable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveWeeklyDisplayFacts {
    pub(crate) remaining_percent: u32,
    pub(crate) provenance: ResetValueProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SelectedCreditConfirmationFacts {
    pub(crate) id_hint: String,
    pub(crate) title: Option<String>,
    pub(crate) expires_unix_seconds: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResetEligibilityDisabledReason {
    LiveInspectionIncomplete,
    WeeklyRemainingNotBelowOnePercent { remaining_percent: u32 },
    NoUsableCredit,
    AuthorityUnavailable,
    PinnedTargetInvalidated(PinnedTargetInvalidationReason),
}

/// Immutable authority-free state published to presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResetWorkflowSnapshot {
    phase: WorkflowPhase,
    confirmation_selection: ConfirmationSelection,
    yes_enabled: bool,
    activities: WorkflowActivities,
    result: Option<WorkflowResult>,
    target: Option<PinnedResetTarget>,
    live_weekly: Option<LiveWeeklyDisplayFacts>,
    credit_inventory: Vec<ResetCreditDisplayRecord>,
    credit_inventory_provenance: Option<ResetValueProvenance>,
    selected_credit: Option<SelectedCreditConfirmationFacts>,
    disabled_yes_reason: Option<ResetEligibilityDisabledReason>,
}

impl ResetWorkflowSnapshot {
    pub(super) fn from_workflow(
        workflow: &ResetWorkflow,
        target: Option<PinnedResetTarget>,
        invalidation_reason: Option<PinnedTargetInvalidationReason>,
    ) -> Self {
        let live_weekly = workflow
            .live_usage_observation()
            .map(|(usage, provenance)| LiveWeeklyDisplayFacts {
                remaining_percent: usage.remaining_percent(),
                provenance: map_provenance(provenance),
            });
        let inventory_observation = workflow.inventory_observation();
        let credit_inventory_provenance = inventory_observation
            .as_ref()
            .map(|(_, provenance)| map_provenance(*provenance));
        let credit_inventory = inventory_observation
            .map(|(inventory, _)| inventory.display_projection())
            .unwrap_or_default()
            .into_iter()
            .map(ResetCreditDisplayRecord::from)
            .collect::<Vec<_>>();
        let selected_credit = credit_inventory
            .iter()
            .find(|credit| credit.earliest_usable)
            .map(|credit| SelectedCreditConfirmationFacts {
                id_hint: credit.id_hint.clone(),
                title: credit.title.clone(),
                expires_unix_seconds: credit.expires_unix_seconds,
            });
        let disabled_yes_reason = disabled_yes_reason(
            workflow,
            live_weekly.as_ref(),
            &credit_inventory,
            invalidation_reason,
        );
        Self {
            phase: workflow.phase(),
            confirmation_selection: workflow.confirmation_selection(),
            yes_enabled: workflow.yes_enabled(),
            activities: workflow.activities().clone(),
            result: workflow.result().cloned(),
            target,
            live_weekly,
            credit_inventory,
            credit_inventory_provenance,
            selected_credit,
            disabled_yes_reason,
        }
    }

    pub(crate) const fn phase(&self) -> WorkflowPhase {
        self.phase
    }

    pub(crate) const fn confirmation_selection(&self) -> ConfirmationSelection {
        self.confirmation_selection
    }

    pub(crate) const fn yes_enabled(&self) -> bool {
        self.yes_enabled
    }

    pub(crate) const fn activities(&self) -> &WorkflowActivities {
        &self.activities
    }

    pub(crate) const fn result(&self) -> Option<&WorkflowResult> {
        self.result.as_ref()
    }

    #[cfg(test)]
    pub(crate) const fn target(&self) -> Option<&PinnedResetTarget> {
        self.target.as_ref()
    }

    pub(crate) const fn live_weekly(&self) -> Option<&LiveWeeklyDisplayFacts> {
        self.live_weekly.as_ref()
    }

    pub(crate) fn credit_inventory(&self) -> &[ResetCreditDisplayRecord] {
        &self.credit_inventory
    }

    pub(crate) const fn credit_inventory_provenance(&self) -> Option<ResetValueProvenance> {
        self.credit_inventory_provenance
    }

    pub(crate) const fn selected_credit(&self) -> Option<&SelectedCreditConfirmationFacts> {
        self.selected_credit.as_ref()
    }

    pub(crate) const fn disabled_yes_reason(&self) -> Option<&ResetEligibilityDisabledReason> {
        self.disabled_yes_reason.as_ref()
    }
}

impl From<ResetCreditDisplayProjection> for ResetCreditDisplayRecord {
    fn from(projection: ResetCreditDisplayProjection) -> Self {
        Self {
            id_hint: projection.id_hint,
            status: match projection.status {
                ResetCreditDisplayStatus::Available => ResetCreditDisplayStatusDto::Available,
                ResetCreditDisplayStatus::Redeeming => ResetCreditDisplayStatusDto::Redeeming,
                ResetCreditDisplayStatus::Redeemed => ResetCreditDisplayStatusDto::Redeemed,
            },
            title: projection.title,
            expires_unix_seconds: projection.expires_unix_seconds,
            earliest_usable: projection.earliest_usable,
        }
    }
}

fn map_provenance(provenance: RenderValueProvenance) -> ResetValueProvenance {
    match provenance {
        RenderValueProvenance::CurrentLive => ResetValueProvenance::CurrentLive,
        RenderValueProvenance::PreviousLiveRefreshing => {
            ResetValueProvenance::PreviousLiveRefreshing
        }
    }
}

fn disabled_yes_reason(
    workflow: &ResetWorkflow,
    live_weekly: Option<&LiveWeeklyDisplayFacts>,
    credit_inventory: &[ResetCreditDisplayRecord],
    invalidation_reason: Option<PinnedTargetInvalidationReason>,
) -> Option<ResetEligibilityDisabledReason> {
    if workflow.yes_enabled() {
        return None;
    }
    if let Some(reason) = invalidation_reason {
        return Some(ResetEligibilityDisabledReason::PinnedTargetInvalidated(
            reason,
        ));
    }
    if workflow.authority_failure().is_some() {
        return Some(ResetEligibilityDisabledReason::AuthorityUnavailable);
    }
    let Some(live_weekly) = live_weekly else {
        return Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete);
    };
    if live_weekly.provenance != ResetValueProvenance::CurrentLive {
        return Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete);
    }
    if live_weekly.remaining_percent >= 1 {
        return Some(
            ResetEligibilityDisabledReason::WeeklyRemainingNotBelowOnePercent {
                remaining_percent: live_weekly.remaining_percent,
            },
        );
    }
    if !credit_inventory.iter().any(|credit| credit.earliest_usable) {
        return Some(ResetEligibilityDisabledReason::NoUsableCredit);
    }
    Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete)
}

/// Render-safe terminal value returned by the command-owned session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResetSessionOutcome {
    Cancelled,
    Finished(WorkflowResult),
}

#[derive(Clone, Debug)]
pub(crate) struct ResetIntentSender {
    sender: mpsc::UnboundedSender<ResetSessionIntent>,
}

impl ResetIntentSender {
    pub(super) const fn new(sender: mpsc::UnboundedSender<ResetSessionIntent>) -> Self {
        Self { sender }
    }

    pub(crate) fn send(
        &self,
        intent: ResetSessionIntent,
    ) -> std::future::Ready<Result<(), mpsc::error::SendError<ResetSessionIntent>>> {
        std::future::ready(self.sender.send(intent))
    }

    pub(crate) fn send_now(
        &self,
        intent: ResetSessionIntent,
    ) -> Result<(), mpsc::error::SendError<ResetSessionIntent>> {
        self.sender.send(intent)
    }
}

pub(crate) struct ResetSessionPorts {
    pub(crate) intent_sender: ResetIntentSender,
    pub(crate) snapshot_receiver: watch::Receiver<ResetWorkflowSnapshot>,
}

#[cfg(test)]
impl ResetSessionPorts {
    pub(crate) fn test_channels(
        initial_snapshot: ResetWorkflowSnapshot,
    ) -> (
        Self,
        mpsc::UnboundedReceiver<ResetSessionIntent>,
        watch::Sender<ResetWorkflowSnapshot>,
    ) {
        let (intent_sender, intent_receiver) = mpsc::unbounded_channel();
        let (snapshot_sender, snapshot_receiver) = watch::channel(initial_snapshot);
        (
            Self {
                intent_sender: ResetIntentSender::new(intent_sender),
                snapshot_receiver,
            },
            intent_receiver,
            snapshot_sender,
        )
    }
}

#[cfg(test)]
impl ResetWorkflowSnapshot {
    pub(crate) fn test_snapshot(
        phase: WorkflowPhase,
        confirmation_selection: ConfirmationSelection,
        activities: WorkflowActivities,
        result: Option<WorkflowResult>,
        live_weekly: Option<LiveWeeklyDisplayFacts>,
        credit_inventory: Vec<ResetCreditDisplayRecord>,
        disabled_yes_reason: Option<ResetEligibilityDisabledReason>,
    ) -> Self {
        let selected_credit = credit_inventory
            .iter()
            .find(|credit| credit.earliest_usable)
            .map(|credit| SelectedCreditConfirmationFacts {
                id_hint: credit.id_hint.clone(),
                title: credit.title.clone(),
                expires_unix_seconds: credit.expires_unix_seconds,
            });
        let yes_enabled = phase == WorkflowPhase::Confirming && disabled_yes_reason.is_none();
        Self {
            phase,
            confirmation_selection,
            yes_enabled,
            activities,
            result,
            target: None,
            live_weekly,
            credit_inventory,
            credit_inventory_provenance: Some(ResetValueProvenance::CurrentLive),
            selected_credit,
            disabled_yes_reason,
        }
    }
}
