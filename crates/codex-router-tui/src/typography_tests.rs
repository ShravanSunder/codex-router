use crate::*;
use iocraft::prelude::*;

#[test]
fn shared_heading_roles_paint_literal_hierarchy() {
    for (level, expected) in [
        (UiHeadingLevel::Surface, Color::Cyan),
        (UiHeadingLevel::Section, Color::White),
        (UiHeadingLevel::Group, Color::Grey),
    ] {
        let canvas = element!(UiHeading(content: "Heading", level)).render(None);
        let style = canvas.cell(0, 0).unwrap().text_style().unwrap();
        assert_eq!(style.color, Some(expected));
        assert_eq!(style.weight, Weight::Bold);
    }
    let canvas =
        element!(UiHeading(content: "Background", level: UiHeadingLevel::Surface, inactive: true))
            .render(None);
    assert_eq!(
        canvas.cell(0, 0).unwrap().text_style().unwrap().color,
        Some(Color::Grey)
    );
}

#[test]
fn shared_text_roles_keep_selection_separate_from_hierarchy() {
    for (role, expected_color, expected_weight) in [
        (UiTextRole::Body, Color::White, Weight::Normal),
        (UiTextRole::Secondary, Color::Grey, Weight::Normal),
        (UiTextRole::Disabled, Color::DarkGrey, Weight::Normal),
        (UiTextRole::Selected, Color::Yellow, Weight::Bold),
    ] {
        let canvas = element!(UiText(content: "Value", role)).render(None);
        let style = canvas.cell(0, 0).unwrap().text_style().unwrap();
        assert_eq!(style.color, Some(expected_color));
        assert_eq!(style.weight, expected_weight);
    }
}

#[test]
fn shared_action_role_is_white_or_distinct_yellow_focus() {
    for has_focus in [false, true] {
        let canvas = element!(DialogAction(label: "Apply", has_focus)).render(None);
        let style = canvas.cell(0, 0).unwrap().text_style().unwrap();
        assert_eq!(
            style.color,
            Some(if has_focus {
                Color::Yellow
            } else {
                Color::White
            })
        );
        assert_eq!(style.weight, Weight::Bold);
        assert_eq!(style.underline, has_focus);
    }
}

#[test]
fn shared_blocked_action_looks_disabled_and_has_no_focus_decoration() {
    let canvas = element! {
        ContextProvider(value: Context::owned(DialogInputScope::Blocked)) {
            DialogAction(label: "Help", has_focus: true)
        }
    }
    .render(None);
    let style = canvas.cell(0, 0).unwrap().text_style().unwrap();
    assert_eq!(style.color, Some(Color::DarkGrey));
    assert!(
        !style.underline,
        "blocked background controls cannot advertise active focus"
    );
}

#[test]
fn surface_header_keeps_help_on_the_title_row_at_phone_width() {
    let frame = element! {
        View(width: 44) {
            SurfaceHeader(title: "Sessions · 6/6", help_action: Some(element!(DialogAction(label: "Help")).into_any()))
        }
    }.render(None).to_string();
    assert_eq!(frame.lines().count(), 1, "{frame}");
    assert!(
        frame.contains("Sessions · 6/6") && frame.contains("Help"),
        "{frame}"
    );
    assert!(!frame.contains("F1"), "{frame}");
}

#[test]
fn secondary_help_action_keeps_distinct_focus_and_blocked_styles() {
    for (has_focus, color, weight) in [
        (false, Color::Grey, Weight::Normal),
        (true, Color::Yellow, Weight::Bold),
    ] {
        let canvas = element!(DialogAction(label: "Help", emphasis: DialogActionEmphasis::Secondary, has_focus)).render(None);
        let style = canvas.cell(0, 0).unwrap().text_style().unwrap();
        assert_eq!(style.color, Some(color));
        assert_eq!(style.weight, weight);
        assert_eq!(style.underline, has_focus);
    }
    let canvas = element! {
        ContextProvider(value: Context::owned(DialogInputScope::Blocked)) {
            DialogAction(label: "Help", emphasis: DialogActionEmphasis::Secondary, has_focus: true)
        }
    }
    .render(None);
    let style = canvas.cell(0, 0).unwrap().text_style().unwrap();
    assert_eq!(style.color, Some(Color::DarkGrey));
    assert!(!style.underline);
}

#[test]
fn surface_header_gap_is_explicit_and_preserves_the_existing_default() {
    for (column_gap, expected) in [
        (None, "3 stale  Details  Help"),
        (Some(1u16), "3 stale Details Help"),
    ] {
        let frame = element! {
            View(width: 44) {
                SurfaceHeader(title: "Quota", column_gap,
                    children: vec![
                        element!(UiText(content: "3 stale", role: UiTextRole::Secondary)).into_any(),
                        element!(DialogAction(label: "Details")).into_any(),
                    ],
                    help_action: Some(element!(DialogAction(label: "Help", emphasis: DialogActionEmphasis::Secondary)).into_any()))
            }
        }.render(None).to_string();
        assert_eq!(frame.lines().count(), 1, "{frame}");
        assert!(
            frame.contains(expected),
            "existing callers keep two cells; compact callers choose one: {frame}"
        );
    }
}
