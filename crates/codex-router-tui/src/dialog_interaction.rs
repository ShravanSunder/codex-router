use iocraft::prelude::*;

/// Whether descendants may act on broadcast terminal input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DialogInputScope {
    #[default]
    Active,
    Blocked,
}

/// Background widgets opt into the host's input policy at their event boundary.
/// Painting an overlay alone does not consume iocraft's broadcast events.
pub trait UseDialogInteraction {
    fn dialog_input_enabled(&self) -> bool;
    fn use_dialog_events(&mut self, handler: impl FnMut(TerminalEvent) + Send + 'static);
    fn use_dialog_local_events(&mut self, handler: impl FnMut(TerminalEvent) + Send + 'static);
}

impl UseDialogInteraction for Hooks<'_, '_> {
    fn dialog_input_enabled(&self) -> bool {
        self.try_use_context::<DialogInputScope>()
            .is_none_or(|scope| *scope == DialogInputScope::Active)
    }

    fn use_dialog_events(&mut self, mut handler: impl FnMut(TerminalEvent) + Send + 'static) {
        let enabled = self.dialog_input_enabled();
        self.use_terminal_events(move |event| {
            if enabled {
                handler(event);
            }
        });
    }

    fn use_dialog_local_events(&mut self, mut handler: impl FnMut(TerminalEvent) + Send + 'static) {
        let enabled = self.dialog_input_enabled();
        self.use_local_terminal_events(move |event| {
            if enabled {
                handler(event);
            }
        });
    }
}
