use crate::UseDialogInteraction;
use iocraft::prelude::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DialogActionEmphasis {
    #[default]
    Primary,
    Secondary,
}

#[derive(Props)]
pub struct DialogActionProps {
    #[iocraft(required)]
    pub label: String,
    pub has_focus: bool,
    pub emphasis: DialogActionEmphasis,
    pub handler: HandlerMut<'static, ()>,
}

/// Native iocraft button with the dialog host's input policy applied.
#[component]
pub fn DialogAction(props: &mut DialogActionProps, hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let enabled = hooks.dialog_input_enabled();
    let mut handler = props.handler.take();
    let (color, weight) = if !enabled {
        (Color::DarkGrey, Weight::Normal)
    } else if props.has_focus {
        (Color::Yellow, Weight::Bold)
    } else {
        match props.emphasis {
            DialogActionEmphasis::Primary => (Color::White, Weight::Bold),
            DialogActionEmphasis::Secondary => (Color::Grey, Weight::Normal),
        }
    };
    element! {
        Button(has_focus: enabled && props.has_focus, handler: move |_| {
            if enabled { handler(()); }
        }) {
            Text(content: props.label.clone(), color, weight,
                decoration: if enabled && props.has_focus { TextDecoration::Underline } else { TextDecoration::None })
        }
    }
}
