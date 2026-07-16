//! Pure correlated quota-reset workflow contracts and reduction.

mod contracts;
mod model;
mod reducer;

pub(in crate::quota_reset) use contracts::CorrelatedOutcome;
pub(crate) use contracts::OperationActivity;
pub(in crate::quota_reset) use contracts::OperationCorrelation;
pub(in crate::quota_reset) use contracts::RenderValueProvenance;
pub(crate) use contracts::WorkflowPhase;
pub(crate) use model::ConfirmationSelection;
pub(in crate::quota_reset) use model::InspectionStart;
pub(crate) use model::OperationSuccess;
pub(in crate::quota_reset) use model::ResetWorkflow;
pub(crate) use model::WorkflowActivities;
pub(in crate::quota_reset) use model::WorkflowIntent;
pub(crate) use model::WorkflowResult;

#[cfg(test)]
mod tests;
