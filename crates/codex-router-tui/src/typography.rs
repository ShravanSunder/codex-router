//! Visual hierarchy is independent of selection and workflow state.
use iocraft::prelude::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiHeadingLevel {
    Surface,
    Section,
    Group,
}

#[derive(Props)]
pub struct UiHeadingProps {
    pub content: String,
    #[iocraft(required)]
    pub level: UiHeadingLevel,
    pub inactive: bool,
}

#[component]
pub fn UiHeading(props: &UiHeadingProps) -> impl Into<AnyElement<'static>> {
    let color = if props.inactive {
        Color::Grey
    } else {
        match props.level {
            UiHeadingLevel::Surface => Color::Cyan,
            UiHeadingLevel::Section => Color::White,
            UiHeadingLevel::Group => Color::Grey,
        }
    };
    element!(Text(content: props.content.clone(), color, weight: Weight::Bold, wrap: TextWrap::NoWrap))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiTextRole {
    Body,
    Secondary,
    Disabled,
    Selected,
}

#[derive(Props)]
pub struct UiTextProps {
    pub content: String,
    #[iocraft(required)]
    pub role: UiTextRole,
    pub wrap: Option<TextWrap>,
}

#[component]
pub fn UiText(props: &UiTextProps) -> impl Into<AnyElement<'static>> {
    let (color, weight) = match props.role {
        UiTextRole::Body => (Color::White, Weight::Normal),
        UiTextRole::Secondary => (Color::Grey, Weight::Normal),
        UiTextRole::Disabled => (Color::DarkGrey, Weight::Normal),
        UiTextRole::Selected => (Color::Yellow, Weight::Bold),
    };
    element!(Text(content: props.content.clone(), color, weight, wrap: props.wrap.unwrap_or(TextWrap::NoWrap)))
}

#[derive(Props)]
pub struct SurfaceHeaderProps<'a> {
    pub title: String,
    pub inactive: bool,
    pub help_action: Option<AnyElement<'a>>,
    pub children: Vec<AnyElement<'a>>,
}

/// A clipped title, optional status and Help share one terminal row.
#[component]
pub fn SurfaceHeader<'a>(props: &mut SurfaceHeaderProps<'a>) -> impl Into<AnyElement<'a>> {
    let help_action = props.help_action.take();
    let children = std::mem::take(&mut props.children);
    element! {
        View(width: 100pct, column_gap: 2, flex_shrink: 0.0_f32) {
            View(flex_grow: 1.0_f32, flex_shrink: 1.0_f32, min_width: 0, overflow: Overflow::Hidden) {
                UiHeading(content: props.title.clone(), level: UiHeadingLevel::Surface, inactive: props.inactive)
            }
            #(children)
            View(flex_shrink: 0.0_f32) { #(help_action) }
        }
    }
}
