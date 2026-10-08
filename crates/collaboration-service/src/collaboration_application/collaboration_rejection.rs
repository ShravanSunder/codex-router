//! The collaboration API's typed rejection reasons, and how each family's failure maps onto them.
//!
//! The reasons are those of the board orchestration design's spec 1 §5 that today's operations
//! can already produce. A family failure that has no counterpart there stays family-specific
//! (`None`); the board-model reasons (writer epochs, moves, roles) arrive with the board model.

/// A typed rejection reason from spec 1 §5.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollaborationRejectionReason {
    /// The API's concurrency limit was reached; nothing was done, and the call may be retried.
    Overloaded,
    /// The board is archived.
    Archived,
    /// A request field failed validation (MV-8).
    InvalidShape,
    /// A pagination cursor whose key or scope can't be verified.
    CursorInvalid,
    /// The subject moved on from the revision the write expected (RC-3).
    StaleRevision,
    /// The same request identity was submitted with different content (RC-4).
    ConflictingRequest,
}

impl CollaborationRejectionReason {
    /// The reason's name in spec 1 §5.
    #[must_use]
    pub const fn spec_name(self) -> &'static str {
        match self {
            Self::Overloaded => "overloaded",
            Self::Archived => "archived",
            Self::InvalidShape => "invalidShape",
            Self::CursorInvalid => "cursorInvalid",
            Self::StaleRevision => "staleRevision",
            Self::ConflictingRequest => "conflictingRequest",
        }
    }
}

/// A family's typed failure, classified against the collaboration API's rejection reasons.
pub trait CollaborationRejection {
    /// The spec 1 §5 reason this failure is, or `None` for a family-specific failure.
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason>;

    /// The failure as published to callers: its error code, summary and typed payload.
    fn published_rejection(&self) -> PublishedRejection;
}

/// A failure as callers receive it: a JSON-RPC error code, a summary and, for typed failures,
/// the payload whose `kind` names the failure. Every transport publishes the same object.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct PublishedRejection {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl PublishedRejection {
    /// The request was refused before the operation ran.
    pub const INVALID_PARAMS: i64 = -32602;
    /// The operation refused or could not complete; the payload says why.
    pub const OPERATION_FAILED: i64 = -32050;
    /// The named subject does not exist.
    pub const NOT_FOUND: i64 = -32002;
    /// The service could not describe its own failure.
    pub const INTERNAL: i64 = -32603;

    /// A failure with its typed payload.
    pub fn typed(code: i64, message: impl Into<String>, data: &impl serde::Serialize) -> Self {
        Self {
            code,
            message: message.into(),
            data: serde_json::to_value(data).ok(),
        }
    }

    /// A failure without a payload.
    pub fn bare(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}
