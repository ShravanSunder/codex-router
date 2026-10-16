//! Caller-owned keys retain button visuals without duplicate modified-Enter effects.
use crate::{DialogAction, DialogKeyboardOwner};
use futures_util::StreamExt;
use iocraft::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn caller_owned_dialog_action_does_not_also_activate_for_modified_enter() {
    for (keyboard_owner, expected_activations) in [
        (DialogKeyboardOwner::Button, 1),
        (DialogKeyboardOwner::Caller, 0),
    ] {
        let activations = Arc::new(AtomicUsize::new(0));
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut key = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
        key.modifiers = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        sender.send(TerminalEvent::Key(key)).unwrap();
        let events = futures_util::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|event| (event, receiver))
        });
        let mut component =
            element!(CallerOwnedActionProbe(activations: activations.clone(), keyboard_owner));
        let mut exit_sent = false;
        let frames = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            component
                .mock_terminal_render_loop(MockTerminalConfig::with_events(events))
                .map(|canvas| {
                    let frame = canvas.to_string();
                    if !exit_sent && frame.contains("Key observed") {
                        let mut exit = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('d'));
                        exit.modifiers = KeyModifiers::CONTROL;
                        sender.send(TerminalEvent::Key(exit)).unwrap();
                        exit_sent = true;
                    }
                    frame
                })
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        assert!(frames.iter().any(|frame| frame.contains("Key observed")));
        assert_eq!(activations.load(Ordering::SeqCst), expected_activations);
    }
}

#[derive(Props)]
struct CallerOwnedActionProbeProps {
    activations: Arc<AtomicUsize>,
    keyboard_owner: DialogKeyboardOwner,
}

#[component]
fn CallerOwnedActionProbe(
    props: &CallerOwnedActionProbeProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut observed = hooks.use_state(|| false);
    let mut exit_requested = hooks.use_state(|| false);
    hooks.use_terminal_events(move |event| match event {
        TerminalEvent::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) => observed.set(true),
        TerminalEvent::Key(KeyEvent {
            code: KeyCode::Char('d'),
            modifiers,
            ..
        }) if modifiers.contains(KeyModifiers::CONTROL) => exit_requested.set(true),
        _ => {}
    });
    if exit_requested.get() {
        system.exit();
    }
    let activations = props.activations.clone();
    element! { View {
        Text(content: if observed.get() { "Key observed" } else { "Waiting" })
        DialogAction(label: "Resume", has_focus: true, keyboard_owner: props.keyboard_owner,
            handler: move |_| { activations.fetch_add(1, Ordering::SeqCst); })
    }}
}
