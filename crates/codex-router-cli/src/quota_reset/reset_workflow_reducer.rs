//! Pure correlated quota-reset workflow contracts and reduction.

mod correlated_effect_contracts;
mod reset_workflow_state;
mod state_transition_reducer;

pub(in crate::quota_reset) use correlated_effect_contracts::CorrelatedOutcome;
pub(crate) use correlated_effect_contracts::OperationActivity;
pub(in crate::quota_reset) use correlated_effect_contracts::OperationCorrelation;
pub(in crate::quota_reset) use correlated_effect_contracts::RenderValueProvenance;
pub(crate) use correlated_effect_contracts::WorkflowPhase;
pub(crate) use reset_workflow_state::ConfirmationSelection;
pub(in crate::quota_reset) use reset_workflow_state::InspectionStart;
pub(crate) use reset_workflow_state::OperationSuccess;
pub(in crate::quota_reset) use reset_workflow_state::ResetWorkflow;
pub(crate) use reset_workflow_state::WorkflowActivities;
pub(in crate::quota_reset) use reset_workflow_state::WorkflowIntent;
pub(crate) use reset_workflow_state::WorkflowResult;

#[cfg(test)]
mod state_transition_test;
