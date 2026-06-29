//! CLI-only terminal presentation helpers.

use iocraft::prelude::*;

pub(crate) mod session_picker;

/// Marker type for the CLI presentation layer's iocraft boundary.
#[allow(dead_code)]
pub(crate) struct TerminalPresentationLayer {
    _terminal_color: Option<Color>,
}

impl TerminalPresentationLayer {
    /// Creates a presentation-layer marker without enabling terminal output.
    #[allow(dead_code)]
    pub(crate) const fn new() -> Self {
        Self {
            _terminal_color: None,
        }
    }
}
