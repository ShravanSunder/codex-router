//! Authority-free command-session ports shared with presentation.

use codex_router_core::ids::AccountId;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::quota_reset::workflow::ConfirmationSelection;
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
}

/// Immutable authority-free state published to presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResetWorkflowSnapshot {
    phase: WorkflowPhase,
    confirmation_selection: ConfirmationSelection,
    yes_enabled: bool,
    activities: WorkflowActivities,
    result: Option<WorkflowResult>,
}

impl ResetWorkflowSnapshot {
    pub(super) fn from_workflow(workflow: &ResetWorkflow) -> Self {
        Self {
            phase: workflow.phase(),
            confirmation_selection: workflow.confirmation_selection(),
            yes_enabled: workflow.yes_enabled(),
            activities: workflow.activities().clone(),
            result: workflow.result().cloned(),
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
}

/// Render-safe terminal value returned by the command-owned session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) enum ResetSessionOutcome {
    Cancelled,
    Finished(WorkflowResult),
}

pub(crate) struct ResetSessionPorts {
    pub(crate) intent_sender: mpsc::Sender<ResetSessionIntent>,
    pub(crate) snapshot_receiver: watch::Receiver<ResetWorkflowSnapshot>,
}
