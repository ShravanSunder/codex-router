use crate::*;
use futures_util::StreamExt;
use iocraft::prelude::*;

#[test]
fn framed_dialog_overlays_without_reducing_background_viewport() {
    let frame = element! {
        DialogHost(width: 40u16, height: 12u16,
            background: vec![element! {
                View(width: 40, height: 12, flex_direction: FlexDirection::Column) {
                    Text(content: "BACKGROUND TOP")
                    View(flex_grow: 1.0_f32) {}
                    Text(content: "BACKGROUND BOTTOM")
                }
            }.into_any()],
            dialog: Some(element! {
                DialogFrame(width: 40u16, height: 12u16, title: "Filters".to_owned(),
                    presentation: DialogPresentation::Framed { maximum_width: 26, maximum_height: 8 },
                    margins: DialogMargins { top: 1, right: 2, bottom: 1, left: 2 },
                    actions: vec![element!(Text(content: "Apply  Cancel")).into_any()]) {
                    Text(content: "Claude")
                }
            }.into_any()))
    }.to_string();
    assert_eq!(frame.lines().count(), 12, "{frame}");
    assert!(
        frame
            .lines()
            .next()
            .is_some_and(|line| line.contains("BACKGROUND TOP")),
        "{frame}"
    );
    assert!(
        frame
            .lines()
            .last()
            .is_some_and(|line| line.contains("BACKGROUND BOTTOM")),
        "{frame}"
    );
    assert!(frame.contains("Filters"), "{frame}");
    assert!(frame.contains("Apply  Cancel"), "{frame}");
    assert!(frame.contains('╭') && frame.contains('╯'), "{frame}");
}

#[test]
fn full_screen_dialog_respects_explicit_margins_and_border() {
    let frame = element! {
        View(width: 30, height: 10) {
        DialogFrame(width: 30u16, height: 10u16, title: "Details".to_owned(),
            presentation: DialogPresentation::FullScreen,
            margins: DialogMargins { top: 1, right: 3, bottom: 2, left: 2 },
            padding: 0u16, border_style: BorderStyle::Double,
            actions: vec![element!(Text(content: "Esc Close")).into_any()]) {
            Text(content: "Selected account")
        }
        }
    }
    .to_string();
    assert_eq!(frame.lines().count(), 10, "{frame}");
    let border_row = frame.lines().nth(1).unwrap_or_default();
    assert!(border_row.starts_with("  ╔"), "{frame}");
    assert_eq!(border_row.trim_start().chars().count(), 25, "{frame}");
    assert!(frame.contains("Esc Close"), "{frame}");
}

#[test]
fn excessive_margins_and_tiny_viewports_remain_bounded() {
    for (width, height) in [(0u16, 0u16), (1, 1), (8, 3), (40, 10)] {
        let frame = element! {
            View(width: u32::from(width), height: u32::from(height), overflow: Overflow::Hidden) {
            DialogFrame(width, height, title: "Details".to_owned(),
                presentation: DialogPresentation::FullScreen,
                margins: DialogMargins { top: 100, right: 100, bottom: 100, left: 100 }) {
                Text(content: "long content")
            }
            }
        }
        .to_string();
        assert!(frame.lines().count() <= usize::from(height), "{frame}");
    }
}

#[derive(Props)]
struct InteractionProbeProps {
    label: String,
}

#[component]
fn InteractionProbe(
    props: &InteractionProbeProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut count = hooks.use_state(|| 0_u32);
    hooks.use_dialog_events(move |event| {
        if matches!(
            event,
            TerminalEvent::Key(KeyEvent {
                code: KeyCode::Char('b'),
                kind: KeyEventKind::Press,
                ..
            })
        ) {
            count.set(count.get() + 1);
        }
    });
    hooks.use_dialog_local_events(move |event| {
        if matches!(
            event,
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                ..
            })
        ) {
            count.set(count.get() + 1);
        }
    });
    element!(Text(content: format!("{}:{}", props.label, count.get())))
}

#[component]
fn DialogJourney(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut open = hooks.use_state(|| true);
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut should_exit = hooks.use_state(|| false);
    let (width, height) = hooks.use_terminal_size();
    hooks.use_terminal_events(move |event| {
        if matches!(
            event,
            TerminalEvent::Key(KeyEvent {
                code: KeyCode::Char('q'),
                ..
            })
        ) {
            should_exit.set(true);
        }
    });
    if should_exit.get() {
        system.exit();
    }
    let dialog = if open.get() {
        Some(element! {
            DialogFrame(width: width.max(40), height: height.max(12), title: "Dialog active".to_owned(),
                presentation: DialogPresentation::Framed { maximum_width: 30, maximum_height: 10 },
                keyboard_scroll: true, on_close: move |_| open.set(false),
                actions: vec![element!(Text(content: "Esc Close")).into_any()]) {
                #((1..=40).map(|index| element!(Text(content: format!("CONTENT {index}")))))
            }
        }.into_any())
    } else {
        None
    };
    element! {
        DialogHost(width: width.max(40), height: height.max(12),
            background: vec![element! {
                View(width: 100pct, height: 100pct, flex_direction: FlexDirection::Column) {
                    InteractionProbe(label: "background".to_owned())
                }
            }.into_any()], dialog)
    }
}

#[tokio::test]
async fn dialog_blocks_background_keys_and_pointer_then_restores_them_after_cancel() {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut ui = element!(DialogJourney);
    let frames = ui.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
    tokio::pin!(frames);
    let mut phase = 0;
    let mut last_frame = String::new();
    let journey = async {
        while let Some(canvas) = frames.next().await {
            let frame = canvas.to_string();
            if phase == 0 && frame.contains("Dialog active") {
                assert!(frame.contains("background:0"), "{frame}");
                sender
                    .send(TerminalEvent::Key(KeyEvent::new(
                        KeyEventKind::Press,
                        KeyCode::Char('b'),
                    )))
                    .unwrap();
                sender
                    .send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                        MouseEventKind::Down(MouseButton::Left),
                        2,
                        0,
                    )))
                    .unwrap();
                sender
                    .send(TerminalEvent::Key(KeyEvent::new(
                        KeyEventKind::Press,
                        KeyCode::End,
                    )))
                    .unwrap();
                phase = 1;
            } else if phase == 1 && frame.contains("CONTENT 40") {
                assert!(
                    frame.contains("background:0"),
                    "background input leaked: {frame}"
                );
                assert!(frame.contains("Esc Close"), "scroll lost actions: {frame}");
                sender
                    .send(TerminalEvent::Key(KeyEvent::new(
                        KeyEventKind::Press,
                        KeyCode::Esc,
                    )))
                    .unwrap();
                phase = 2;
            } else if phase == 2 && !frame.contains("Dialog active") {
                assert!(
                    frame.contains("background:0"),
                    "cancel leaked its event: {frame}"
                );
                sender
                    .send(TerminalEvent::Key(KeyEvent::new(
                        KeyEventKind::Press,
                        KeyCode::Char('b'),
                    )))
                    .unwrap();
                phase = 3;
            } else if phase == 3 && frame.contains("background:1") {
                sender
                    .send(TerminalEvent::Key(KeyEvent::new(
                        KeyEventKind::Press,
                        KeyCode::Char('q'),
                    )))
                    .unwrap();
                phase = 4;
            }
            last_frame = frame;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), journey)
        .await
        .unwrap();
    assert_eq!(phase, 4, "{last_frame}");
}
