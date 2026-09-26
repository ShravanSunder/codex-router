//! Pure, shared vocabulary for Router-owned agent sessions.

mod approval_choice;
mod capability_report;
mod interaction_request;
mod session_event;
pub mod session_profile_codec;
mod session_state;

pub use approval_choice::*;
pub use capability_report::*;
pub use interaction_request::*;
pub use message_board::{Identity, SessionRef};
pub use session_event::*;
pub use session_state::*;
