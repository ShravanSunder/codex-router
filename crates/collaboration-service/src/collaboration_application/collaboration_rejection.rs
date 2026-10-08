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
}
