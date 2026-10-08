use crate::LinkApprovalOutcome;
use serde::{Deserialize, Serialize};
use session_event_model::QuestionResponse;

/// A decision payload alone establishes no application or acknowledgment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "camelCase", deny_unknown_fields)]
pub enum LinkDecision {
    Approval(LinkApprovalOutcome),
    Question(QuestionResponse),
}
