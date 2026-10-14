//! The chooser owns keys while open, before browse or machine shortcuts.
use super::{picker_model::SessionsPickerModel, provider_filter_model::ProviderFilterFocus};
use iocraft::prelude::{KeyCode, KeyModifiers};

pub(super) fn handle_provider_filter_key(
    model: &mut SessionsPickerModel,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    if let Some(draft) = model.provider_filter_draft.as_mut() {
        match code {
            KeyCode::Up => draft.move_provider(-1),
            KeyCode::Down => draft.move_provider(1),
            KeyCode::Tab => draft.move_focus(modifiers.contains(KeyModifiers::SHIFT)),
            KeyCode::BackTab => draft.move_focus(true),
            KeyCode::Esc => model.provider_filter_draft = None,
            KeyCode::Enter
                if !modifiers.intersects(
                    KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER,
                ) =>
            {
                if draft.focus == ProviderFilterFocus::Cancel {
                    model.provider_filter_draft = None;
                } else {
                    model.apply_provider_filters();
                }
            }
            _ => {}
        }
        return true;
    }
    if code == KeyCode::F(3)
        || code == KeyCode::Char('f') && modifiers.contains(KeyModifiers::CONTROL)
    {
        model.open_provider_filters();
        return true;
    }
    false
}
