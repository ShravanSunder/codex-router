use crossterm::event::MouseButton;
use iocraft::prelude::*;

#[derive(Default, Props)]
pub(super) struct InteractiveSessionChoiceRowProps<'a> {
    pub(super) children: Vec<AnyElement<'a>>,
    pub(super) focus_handler: HandlerMut<'static, ()>,
    pub(super) activation_handler: HandlerMut<'static, ()>,
    pub(super) activates_on_click: bool,
}

#[component]
pub(super) fn InteractiveSessionChoiceRow<'a>(
    props: &mut InteractiveSessionChoiceRowProps<'a>,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'a>> {
    hooks.use_local_terminal_events({
        let mut focus_handler = props.focus_handler.take();
        let mut activation_handler = props.activation_handler.take();
        let activates_on_click = props.activates_on_click;
        move |event| match event {
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent {
                kind: MouseEventKind::Moved,
                ..
            }) => focus_handler(()),
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                ..
            }) => {
                focus_handler(());
                if activates_on_click {
                    activation_handler(());
                }
            }
            _ => {}
        }
    });

    match props.children.iter_mut().next() {
        Some(child) => child.into(),
        None => element!(View).into_any(),
    }
}
