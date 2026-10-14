use crate::UseDialogInteraction;
use iocraft::prelude::*;

#[derive(Props)]
pub struct DialogActionProps {
    #[iocraft(required)]
    pub label: String,
    pub has_focus: bool,
    pub handler: HandlerMut<'static, ()>,
}

/// Native iocraft button with the dialog host's input policy applied.
#[component]
pub fn DialogAction(props: &mut DialogActionProps, hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let enabled = hooks.dialog_input_enabled();
    let mut handler = props.handler.take();
    element! {
        Button(has_focus: enabled && props.has_focus, handler: move |_| {
            if enabled { handler(()); }
        }) {
            Text(content: props.label.clone(), color: if props.has_focus { Color::Yellow } else { Color::Cyan }, weight: Weight::Bold)
        }
    }
}
