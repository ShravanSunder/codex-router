//! Callers own shortcut meaning and Help state; this component owns presentation.
use crate::{DialogFrame, DialogMargins, DialogPresentation, UiHeading, UiHeadingLevel};
use iocraft::prelude::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutEntry {
    pub keys: String,
    pub description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutSection {
    pub title: String,
    pub entries: Vec<ShortcutEntry>,
}

#[derive(Props)]
pub struct ShortcutHelpProps<'a> {
    #[iocraft(required)]
    pub width: u16,
    #[iocraft(required)]
    pub height: u16,
    pub title: String,
    pub sections: Vec<ShortcutSection>,
    pub actions: Vec<AnyElement<'a>>,
}

#[component]
pub fn ShortcutHelp<'a>(props: &mut ShortcutHelpProps<'a>) -> impl Into<AnyElement<'a>> {
    let sections = std::mem::take(&mut props.sections);
    let actions = std::mem::take(&mut props.actions);
    let key_width = usize::from(props.width).saturating_sub(12).min(16);
    let stacked = props.width < 40;
    let content = sections.into_iter().map(|section| {
        let entries = section.entries.into_iter().map(|entry| {
            element! {
                View(width: 100pct, column_gap: 1, flex_direction: if stacked { FlexDirection::Column } else { FlexDirection::Row }, flex_shrink: 0.0_f32) {
                    View(width: if stacked { Size::Auto } else { Size::Length(key_width as u32) }, flex_shrink: 0.0_f32) {
                        Text(content: entry.keys, color: Color::Grey, weight: Weight::Bold)
                    }
                    View(flex_grow: 1.0_f32, flex_shrink: 1.0_f32, min_width: 0) {
                        Text(content: entry.description, color: Color::White)
                    }
                }
            }.into_any()
        }).collect::<Vec<_>>();
        element! {
            View(width: 100pct, flex_direction: FlexDirection::Column, flex_shrink: 0.0_f32, margin_bottom: 1) {
                UiHeading(content: section.title, level: UiHeadingLevel::Section)
                #(entries)
            }
        }.into_any()
    }).collect::<Vec<_>>();
    element! {
        DialogFrame(width: props.width, height: props.height, title: props.title.clone(),
            presentation: if props.width < 90 || props.height < 28 {
                DialogPresentation::FullScreen
            } else {
                DialogPresentation::Framed { maximum_width: 80, maximum_height: 28 }
            },
            margins: DialogMargins { top: 1, right: 2, bottom: 1, left: 2 },
            keyboard_scroll: true, actions) {
            #(content)
        }
    }
}
