//! ProviderLink value domains. Envelope and endpoint contracts belong to their later consumers.
//!
//! The lifetime authorities stay distinct even when their UUID bytes coincide.
//! ```compile_fail
//! let _: provider_link_protocol::LinkEpoch =
//!     provider_link_protocol::ProviderHostIncarnation::fresh();
//! ```
//! ```compile_fail
//! let _: provider_link_protocol::LinkEpoch = provider_link_protocol::InteractionId::fresh();
//! ```

mod link_identity_error;
pub use link_identity_error::LinkIdentityError;
mod provider_host_incarnation;
pub use provider_host_incarnation::ProviderHostIncarnation;
mod link_epoch;
pub use link_epoch::LinkEpoch;
mod interaction_id;
pub use interaction_id::InteractionId;
mod event_sequence;
pub use event_sequence::{EventSeq, EventSeqError};
mod link_request_id;
pub use link_request_id::{LinkRequestId, LinkRequestIdError};
mod provider_id;
pub use provider_id::{ProviderId, ProviderIdError};
mod link_role;
pub use link_role::LinkRole;
