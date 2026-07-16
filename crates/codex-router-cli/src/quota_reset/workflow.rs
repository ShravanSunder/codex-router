//! Pure correlated quota-reset workflow contracts and reduction.

mod contracts;
mod model;
mod reducer;

pub(in crate::quota_reset) use contracts::CorrelatedOutcome;
pub(in crate::quota_reset) use contracts::OperationCorrelation;
pub(in crate::quota_reset) use contracts::RenderValueProvenance;
pub(in crate::quota_reset) use contracts::WorkflowPhase;
pub(in crate::quota_reset) use model::ConfirmationSelection;
pub(in crate::quota_reset) use model::InspectionStart;
pub(in crate::quota_reset) use model::ResetWorkflow;
pub(in crate::quota_reset) use model::WorkflowActivities;
pub(in crate::quota_reset) use model::WorkflowIntent;
pub(in crate::quota_reset) use model::WorkflowResult;

#[cfg(test)]
mod tests;
