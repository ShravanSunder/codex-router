//! Shortcut content and caller-owned Help action; shared chrome owns visual hierarchy.
use super::picker_model::SessionsPickerModel;
use codex_router_tui::{
    DialogAction, DialogActionEmphasis, ShortcutEntry, ShortcutHelp, ShortcutSection,
};
use iocraft::prelude::*;

pub(super) fn help_action(mut model: State<SessionsPickerModel>) -> AnyElement<'static> {
    element!(DialogAction(label: "Help", emphasis: DialogActionEmphasis::Secondary, handler: move |_| model.write().show_help = true))
        .into_any()
}

pub(super) fn render_help(
    model: &SessionsPickerModel,
    mut state: State<SessionsPickerModel>,
    height: usize,
) -> AnyElement<'static> {
    let entries = |items: &[(&str, &str)]| {
        items
            .iter()
            .map(|(keys, description)| ShortcutEntry {
                keys: (*keys).to_owned(),
                description: (*description).to_owned(),
            })
            .collect()
    };
    let mut sections = vec![ShortcutSection {
        title: "Sessions".to_owned(),
        entries: entries(&[
            ("↑ / ↓", "Move selection"),
            ("PgUp / PgDn", "Move a page"),
            ("Home / End", "First / last choice"),
            ("Enter / F4", "Open details"),
            ("Ctrl+Enter", "Resume selected session"),
            ("Ctrl+Shift+Enter", "Fork selected session"),
            ("Ctrl+N", "New Codex session"),
            ("Ctrl+G / F2", "Choose machine"),
            ("Ctrl+F / F3", "Session filters"),
            ("Ctrl+S", "Cycle scope"),
            ("Ctrl+T", "Cycle runtime view"),
            ("Ctrl+O", "Cycle sort"),
            ("Ctrl+R", "Refresh inventory"),
            (
                "id: / b: / repo:",
                "Search by session ID, branch or repository",
            ),
            ("Typing / Backspace", "Search sessions"),
            ("Esc", "Clear search or go back"),
        ]),
    }];
    if model.provider_filter_draft.is_some() {
        sections.push(ShortcutSection {
            title: "Session filters".to_owned(),
            entries: entries(&[
                ("↑ / ↓", "Choose provider draft"),
                ("Tab / Shift+Tab", "Move between provider, Apply and Cancel"),
                ("Enter", "Apply draft, or cancel when Cancel is focused"),
                ("Esc", "Cancel draft"),
            ]),
        });
    }
    if model.details_dialog.is_some() {
        sections.push(ShortcutSection {
            title: "Session details".to_owned(),
            entries: entries(&[
                ("↑ / ↓ / PgUp / PgDn", "Scroll details"),
                ("Tab / Shift+Tab", "Choose action"),
                ("Enter / Space", "Activate selected action"),
                ("Ctrl+Enter", "Resume selected session"),
                ("Ctrl+Shift+Enter", "Fork selected session"),
                (
                    "Copy session ID",
                    "Send the exact ID to the terminal clipboard",
                ),
                ("Esc", "Close details"),
            ]),
        });
    }
    if model.fork_confirmation.is_some()
        || !matches!(
            model.machine_controls.stage,
            super::picker_machine_controls::PickerMachineStage::Browsing
        )
    {
        sections.push(ShortcutSection {
            title: "Machine and fork choices".to_owned(),
            entries: entries(&[
                ("↑ / ↓", "Choose destination"),
                ("Enter", "Confirm choice"),
                ("Esc", "Go back without confirming"),
            ]),
        });
    }
    // The dialog's controls must precede the longer browse reference on phones.
    let browse_section = sections.remove(0);
    sections.push(browse_section);
    sections.push(ShortcutSection {
        title: "Session status".to_owned(),
        entries: [
            crate::picker_runtime_status::PickerRuntimeStatus::Unknown,
            crate::picker_runtime_status::PickerRuntimeStatus::NotLoaded,
            crate::picker_runtime_status::PickerRuntimeStatus::Blocked,
            crate::picker_runtime_status::PickerRuntimeStatus::Active,
            crate::picker_runtime_status::PickerRuntimeStatus::Idle,
            crate::picker_runtime_status::PickerRuntimeStatus::SystemError,
        ]
        .into_iter()
        .map(|status| ShortcutEntry {
            keys: status.icon().to_owned(),
            description: status.label().to_owned(),
        })
        .collect(),
    });
    sections.push(ShortcutSection {
        title: "Help".to_owned(),
        entries: entries(&[
            ("F1 / Ctrl+/", "Open or close Help"),
            ("Esc", "Close Help"),
            ("Ctrl+C / Ctrl+D", "Exit picker"),
        ]),
    });
    element!(ShortcutHelp(width: u16::try_from(model.width).unwrap_or(u16::MAX), height: u16::try_from(height).unwrap_or(u16::MAX),
        title: "Sessions Help".to_owned(), sections,
        actions: vec![element!(DialogAction(label: "Close", handler: move |_| state.write().show_help = false)).into_any()])).into_any()
}

/// Machine and fork views are pure backgrounds, so Help adds exactly one host.
pub(super) fn render_choice_surface(
    model: &SessionsPickerModel,
    state: State<SessionsPickerModel>,
    height: usize,
) -> Option<Element<'static, View>> {
    let background = if let Some(confirmation) = &model.fork_confirmation {
        super::picker_fork_view::render_fork_confirmation(
            confirmation,
            model.width,
            height,
            help_action(state),
            model.show_help || model.source_action.pending.is_some(),
        )
        .into_any()
    } else if !matches!(
        model.machine_controls.stage,
        super::picker_machine_controls::PickerMachineStage::Browsing
    ) {
        super::picker_machine_view::render_machine_choices(model, height, help_action(state))
            .into_any()
    } else {
        return None;
    };
    let dialog = if let Some(pending) = &model.source_action.pending {
        Some(super::source_action_view::checking_dialog(
            pending, state, height,
        ))
    } else {
        model.show_help.then(|| render_help(model, state, height))
    };
    Some(element! {
        View(width: model.width as u32, height: height as u32) {
            codex_router_tui::DialogHost(width: u16::try_from(model.width).unwrap_or(u16::MAX), height: u16::try_from(height).unwrap_or(u16::MAX), background: vec![background, element!(View(position: Position::Absolute, bottom: 2, left: 2) { #(super::source_action_view::source_error(model)) }).into_any()], dialog)
        }
    })
}
