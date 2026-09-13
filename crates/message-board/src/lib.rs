//! Typed project message-board contracts and validated domain values.

mod board_failures;
mod board_identity;
mod board_messages;
mod board_metadata;
mod board_operations;
mod board_pagination;
mod repository_identity;

pub use board_failures::*;
pub use board_identity::*;
pub use board_messages::*;
pub use board_metadata::*;
pub use board_operations::*;
pub use board_pagination::*;
pub use repository_identity::*;
