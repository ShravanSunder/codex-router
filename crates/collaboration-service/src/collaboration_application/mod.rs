//! The typed collaboration application service.
//!
//! One public async method per collaboration operation, grouped by family. Each family takes
//! the existing typed `collaboration-protocol` and `message-board` requests and returns their
//! typed results or the family's typed failure. Transport adapters only decode requests, call
//! these methods and encode what comes back; no operation logic lives in a transport.
//!
//! Each family is a handle that borrows the Router's dependencies, handed out by
//! [`CollaborationApplication`]: board (`board_operations`); messages (`message_operations`);
//! conversations (`conversation_operations`); wakes (`wake_operations`); schedules, runs,
//! instructions and automation inspection (`automation_operations`, `schedule_operations`,
//! `automation_inspection_operations`, `automation_history_operations`); approvals and
//! questions (`interaction_operations`); sessions, the journal and the address book
//! (`session_operations`, `journal_operations`); provider observation
//! (`provider_observation_operations`). Heavier paging logic stays in the crate's domain modules
//! (`codex_session_inventory`, `provider_session_inventory`, `schedule_preparation`).
#![expect(
    clippy::result_large_err,
    reason = "typed failures are the published payloads, returned once per IO-bound request"
)]
mod automation_history_operations;
mod automation_inspection_operations;
mod automation_operations;
mod board_operations;
mod collaboration_rejection;
mod conversation_operations;
mod interaction_operations;
mod journal_operations;
mod message_operations;
mod provider_observation_operations;
mod schedule_operations;
mod session_operations;
mod wake_operations;

pub use automation_operations::AutomationOperations;
pub use board_operations::{BoardOperations, ThreadWaitBudgetError, thread_wait_root_notice_limit};
pub use collaboration_rejection::{
    CollaborationRejection, CollaborationRejectionReason, PublishedRejection,
};
pub use conversation_operations::{ConversationFailure, ConversationOperations};
pub use interaction_operations::{InteractionFailure, InteractionOperations, QuestionRejection};
pub use journal_operations::{JournalCursorInvalidation, JournalFailure, JournalUnavailableKind};
pub use message_operations::{
    MessageFailure, MessageFailureKind, MessageFailureStage, MessageOperations,
};
pub use provider_observation_operations::{ObservationFailure, ObservationOperations};
pub use schedule_operations::ScheduleOperationFailure;
pub(crate) use session_operations::INVALID_INVENTORY_PARAMETERS;
pub use session_operations::{
    EndpointDirectoryUnavailable, NativeSessionFailure, NativeSessionFailureKind,
    NativeSessionStage, ProviderInventoryFailure, ProviderInventoryFailureKind, SessionOperations,
};
#[cfg(test)]
pub(crate) use session_operations::{
    classify_native_call_failure, rename_method_unsupported, valid_session_rename_name,
};
pub use wake_operations::{WakeOperations, WakeWaitFailure};

use crate::ServiceIdentity;

/// The application service over one Router's composed collaboration dependencies.
#[derive(Clone)]
pub struct CollaborationApplication {
    identity: ServiceIdentity,
}

impl CollaborationApplication {
    #[must_use]
    pub fn new(identity: ServiceIdentity) -> Self {
        Self { identity }
    }

    /// The Router this application serves.
    #[must_use]
    pub fn service_id(&self) -> &collaboration_protocol::UuidIdentity {
        &self.identity.service_id
    }

    /// This run of the Router; a restart starts a new epoch.
    #[must_use]
    pub fn service_epoch(&self) -> &collaboration_protocol::UuidIdentity {
        &self.identity.service_epoch
    }

    /// The machine label the Router publishes and writes into push lines.
    #[must_use]
    pub fn machine_label(&self) -> &collaboration_protocol::MachineLabel {
        self.identity.machine_identity.machine_label()
    }

    /// Projects, boards, topics, threads, posts, inbox and thread subscriptions.
    #[must_use]
    pub fn board(&self) -> BoardOperations<'_> {
        BoardOperations::new(&self.identity)
    }

    /// Provider conversations and recorded Codex conversation operations.
    #[must_use]
    pub fn conversations(&self) -> ConversationOperations<'_> {
        ConversationOperations::new(&self.identity)
    }

    /// Direct messages: sends, replies, push inspection, inbox and history.
    #[must_use]
    pub fn messages(&self) -> MessageOperations<'_> {
        MessageOperations::new(&self.identity)
    }

    /// Provider Session observation.
    #[must_use]
    pub fn observation(&self) -> ObservationOperations<'_> {
        ObservationOperations::new(&self.identity)
    }

    /// Codex and provider sessions, the lifecycle journal and the address book.
    #[must_use]
    pub fn sessions(&self) -> SessionOperations<'_> {
        SessionOperations::new(&self.identity)
    }

    /// Automation configuration, runs, instructions, schedules and automation inspection.
    #[must_use]
    pub fn automation(&self) -> AutomationOperations<'_> {
        AutomationOperations::new(&self.identity)
    }

    /// Wakes and their deliveries.
    #[must_use]
    pub fn wakes(&self) -> WakeOperations<'_> {
        WakeOperations::new(&self.identity.service_id, self.identity.automation.as_ref())
            .with_wait_capacity(&self.identity.wake_wait_permits)
    }

    /// Approvals and questions held by the interaction broker.
    #[must_use]
    pub fn interactions(&self) -> InteractionOperations<'_> {
        InteractionOperations::new(&self.identity)
    }
}

/// The most bytes of encoded JSON any operation returns, whatever transport carries it.
pub const RESULT_LIMIT_BYTES: usize = 1024 * 1024;

/// The response bound for transports that frame a result apart from its envelope.
pub const API_RESULT_BUDGET: ResultByteBudget = ResultByteBudget::new(RESULT_LIMIT_BYTES, 0);

/// The response bound an operation's result must fit.
///
/// A response is the encoded result inside its transport's envelope. Paged operations trim
/// their page until the response fits; single results that cannot fit fail. The bound belongs
/// to the caller's transport, so the same operation serves any carrier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResultByteBudget {
    response_limit_bytes: usize,
    envelope_bytes: usize,
}

impl ResultByteBudget {
    /// A response of at most `response_limit_bytes`, of which `envelope_bytes` frame the result.
    #[must_use]
    pub const fn new(response_limit_bytes: usize, envelope_bytes: usize) -> Self {
        Self {
            response_limit_bytes,
            envelope_bytes,
        }
    }

    #[must_use]
    pub const fn response_limit_bytes(self) -> usize {
        self.response_limit_bytes
    }

    /// The bytes of the whole response carrying `result`, or `None` when it cannot be encoded.
    #[must_use]
    pub fn response_bytes<TResult: serde::Serialize>(self, result: &TResult) -> Option<usize> {
        serde_json::to_vec(result)
            .ok()
            .map(|bytes| bytes.len().saturating_add(self.envelope_bytes))
    }

    /// Whether the response carrying `result` fits. An unencodable result never fits.
    #[must_use]
    pub fn admits<TResult: serde::Serialize>(self, result: &TResult) -> bool {
        self.response_bytes(result)
            .is_some_and(|bytes| bytes <= self.response_limit_bytes)
    }
}

#[cfg(test)]
#[path = "collaboration_rejection_tests.rs"]
mod collaboration_rejection_tests;

#[cfg(test)]
#[path = "result_byte_budget_tests.rs"]
mod result_byte_budget_tests;
