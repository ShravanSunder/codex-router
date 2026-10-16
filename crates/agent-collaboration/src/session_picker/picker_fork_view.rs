//! Content-sized fork confirmation using the picker's existing iocraft styling.
use super::picker_fork_confirmation::ForkConfirmation;
use codex_router_tui::{SurfaceHeader, UiHeading, UiHeadingLevel};
use iocraft::prelude::*;

pub(super) fn render_fork_confirmation(
    confirmation: &ForkConfirmation,
    width: usize,
    height: usize,
    help_action: AnyElement<'static>,
    inactive: bool,
) -> Element<'static, View> {
    let available_rows = height.saturating_sub(13).max(1);
    let window_start = confirmation
        .focused_destination
        .saturating_sub(available_rows.saturating_sub(1));
    let rows = confirmation.destinations.iter().enumerate().skip(window_start).take(available_rows).map(|(index, destination)| {
        let focused = index == confirmation.focused_destination;
        element! {
            View(column_gap: 1) {
                Text(content: if focused { "❯" } else { " " }, color: Color::Yellow)
                Text(content: destination.label.clone(), color: if focused { Color::Yellow } else { Color::Grey })
                #(destination.availability.explanation().map(|_| element! { Text(content: "Unavailable", color: Color::DarkGrey) }))
            }
        }.into_any()
    }).collect::<Vec<_>>();
    let explanation = confirmation.notice.or_else(|| {
        confirmation
            .destinations
            .get(confirmation.focused_destination)
            .and_then(|destination| destination.availability.explanation())
    });
    element! {
        View(width: width as u32, height: height as u32, padding_left: 1, padding_right: 1,
            flex_direction: FlexDirection::Column, border_style: BorderStyle::Round,
            border_color: Color::Cyan, overflow: Overflow::Hidden) {
            SurfaceHeader(title: "Fork session".to_owned(), inactive, help_action: Some(help_action))
            View(border_style: BorderStyle::Single, border_color: Color::DarkGrey, padding: 1,
                margin_top: 1, flex_direction: FlexDirection::Column) {
                UiHeading(content: confirmation.title.clone(), level: UiHeadingLevel::Section)
                Text(content: format!("Source: {}", confirmation.source_label), color: Color::Grey)
                Text(content: format!("Working directory: {}", confirmation.directory_label), color: Color::Grey)
                Text(content: format!("Directory origin: {}", confirmation.directory_origin), color: Color::DarkGrey)
                View(margin_top: 1) { UiHeading(content: "Destination".to_owned(), level: UiHeadingLevel::Group) }
                #(rows)
            }
            #(explanation.map(|message| element! { Text(content: message, color: Color::Yellow) }))
            View(flex_grow: 1.0_f32) {}

        }
    }
}
