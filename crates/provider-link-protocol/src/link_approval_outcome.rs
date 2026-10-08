use serde::{Deserialize, Serialize};

/// Offered-option validation remains with the E11 interaction owner.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LinkApprovalOutcome {
    Selected {
        option_id: String,
        note: Option<String>,
    },
    Cancelled,
    Unavailable,
}
