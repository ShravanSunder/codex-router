//! Shared terminal presentation. Product state and effects remain with callers.
mod dialog;
mod dialog_interaction;

pub use dialog::{
    DialogFrame, DialogFrameProps, DialogHost, DialogHostProps, DialogMargins, DialogPresentation,
};
pub use dialog_interaction::{DialogInputScope, UseDialogInteraction};
mod dialog_action;
pub use dialog_action::{DialogAction, DialogActionProps};

#[cfg(test)]
mod dialog_async_tests;
#[cfg(test)]
mod dialog_tests;
