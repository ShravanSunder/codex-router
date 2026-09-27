//! Pure, shared vocabulary for Router-owned agent sessions.

mod approval_choice;
mod capability_report;
mod input_identity;
mod interaction_request;
mod prompt_content;
mod question_response;
mod session_event;
pub mod session_profile_codec;
mod session_state;

pub use approval_choice::*;
pub use capability_report::*;
pub use input_identity::*;
pub use interaction_request::*;
pub use message_board::{Identity, SessionRef};
pub use prompt_content::*;
pub use question_response::*;
pub use session_event::*;
pub use session_state::*;
