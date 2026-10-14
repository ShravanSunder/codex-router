use crate::DialogInputScope;
use iocraft::prelude::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogPresentation {
    FullScreen,
    Framed {
        maximum_width: u16,
        maximum_height: u16,
    },
}

/// Vertical values are terminal rows; horizontal values are terminal columns.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DialogMargins {
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
    pub left: u16,
}

#[derive(Props)]
pub struct DialogHostProps<'a> {
    #[iocraft(required)]
    pub width: u16,
    #[iocraft(required)]
    pub height: u16,
    pub background: Vec<AnyElement<'a>>,
    pub dialog: Option<AnyElement<'a>>,
}

/// Keeps background state mounted while preventing participating widgets from
/// acting on input. Caller-level browse dispatch must also honor its modal state.
#[component]
pub fn DialogHost<'a>(props: &mut DialogHostProps<'a>) -> impl Into<AnyElement<'a>> {
    let scope = if props.dialog.is_some() {
        DialogInputScope::Blocked
    } else {
        DialogInputScope::Active
    };
    let background = std::mem::take(&mut props.background);
    let dialog = props.dialog.take();
    element! {
        View(width: u32::from(props.width), height: u32::from(props.height), overflow: Overflow::Hidden) {
            ContextProvider(value: Context::owned(scope)) { #(background) }
            ContextProvider(value: Context::owned(DialogInputScope::Active)) { #(dialog) }
        }
    }
}

#[derive(Props)]
pub struct DialogFrameProps<'a> {
    #[iocraft(required)]
    pub presentation: DialogPresentation,
    #[iocraft(required)]
    pub width: u16,
    #[iocraft(required)]
    pub height: u16,
    #[iocraft(required)]
    pub title: String,
    pub margins: DialogMargins,
    pub padding: Option<u16>,
    pub border_style: Option<BorderStyle>,
    pub border_color: Option<Color>,
    pub keyboard_scroll: bool,
    pub on_close: HandlerMut<'static, ()>,
    pub children: Vec<AnyElement<'a>>,
    pub actions: Vec<AnyElement<'a>>,
}

/// Composable dialog chrome: fixed title/actions and a library-managed scrolling
/// body. Domain selection, commit/cancel, loading and focus belong to callers.
#[component]
pub fn DialogFrame<'a>(
    props: &mut DialogFrameProps<'a>,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'a>> {
    hooks.use_terminal_events({
        let mut on_close = props.on_close.take();
        move |event| {
            if matches!(
                event,
                TerminalEvent::Key(KeyEvent {
                    code: KeyCode::Esc,
                    kind: KeyEventKind::Press,
                    ..
                })
            ) {
                on_close(());
            }
        }
    });
    let left = props.margins.left.min(props.width);
    let right = props.margins.right.min(props.width.saturating_sub(left));
    let top = props.margins.top.min(props.height);
    let bottom = props.margins.bottom.min(props.height.saturating_sub(top));
    let available_width = props.width.saturating_sub(left + right);
    let available_height = props.height.saturating_sub(top + bottom);
    let (frame_width, frame_height) = match props.presentation {
        DialogPresentation::FullScreen => (available_width, available_height),
        DialogPresentation::Framed {
            maximum_width,
            maximum_height,
        } => (
            maximum_width.min(available_width),
            maximum_height.min(available_height),
        ),
    };
    let children = std::mem::take(&mut props.children);
    let actions = std::mem::take(&mut props.actions);
    element! {
        View(
            position: Position::Absolute, top: 0, left: 0,
            width: u32::from(props.width), height: u32::from(props.height),
            padding_left: left, padding_right: right, padding_top: top, padding_bottom: bottom,
            align_items: AlignItems::Center, justify_content: JustifyContent::Center,
            overflow: Overflow::Hidden,
        ) {
            View(
                width: u32::from(frame_width), height: u32::from(frame_height),
                flex_direction: FlexDirection::Column, overflow: Overflow::Hidden,
                border_style: props.border_style.unwrap_or(BorderStyle::Round),
                border_color: props.border_color.unwrap_or(Color::Cyan),
                background_color: Color::Reset, padding: props.padding.unwrap_or(1),
                flex_shrink: 0.0_f32,
            ) {
                View(flex_shrink: 0.0_f32) { Text(content: props.title.clone(), weight: Weight::Bold, color: Color::Cyan, wrap: TextWrap::NoWrap) }
                View(flex_grow: 1.0_f32, flex_shrink: 1.0_f32, min_height: 0) {
                    ScrollView(keyboard_scroll: props.keyboard_scroll) {
                        View(width: 100pct, flex_direction: FlexDirection::Column, flex_shrink: 0.0_f32) { #(children) }
                    }
                }
                View(flex_shrink: 0.0_f32, flex_direction: FlexDirection::Column) { #(actions) }
            }
        }
    }
}
