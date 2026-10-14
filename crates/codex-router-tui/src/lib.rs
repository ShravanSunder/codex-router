//! Shared terminal presentation. Product state and effects remain with callers.
mod dialog;
mod dialog_interaction;

pub use dialog::{
    DialogFrame, DialogFrameProps, DialogHost, DialogHostProps, DialogMargins, DialogPresentation,
};
pub use dialog_interaction::{DialogInputScope, UseDialogInteraction};
mod dialog_action;
pub use dialog_action::{DialogAction, DialogActionProps};
mod typography;
pub use typography::{
    SurfaceHeader, SurfaceHeaderProps, UiHeading, UiHeadingLevel, UiHeadingProps, UiText,
    UiTextProps, UiTextRole,
};
mod shortcut_help;
pub use shortcut_help::{ShortcutEntry, ShortcutHelp, ShortcutHelpProps, ShortcutSection};

#[cfg(test)]
mod dialog_async_tests;
#[cfg(test)]
mod dialog_tests;
#[cfg(test)]
mod typography_tests;
