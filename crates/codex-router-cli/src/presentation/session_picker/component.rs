use std::io;

use iocraft::prelude::*;

use crate::presentation::session_picker::action::SessionsPickerKey;
use crate::presentation::session_picker::action::SessionsPickerOutcome;
use crate::presentation::session_picker::model::SessionsPickerModel;
use crate::presentation::session_picker::render::MIN_PICKER_WIDTH;
use crate::presentation::session_picker::request::SessionsPickerRequest;

#[derive(Default, Props)]
pub(crate) struct SessionsPickerComponentProps<'a> {
    request: SessionsPickerRequest,
    width: usize,
    selected_outcome_out: Option<&'a mut Option<SessionsPickerOutcome>>,
}

#[component]
pub(crate) fn SessionsPickerComponent<'a>(
    props: &mut SessionsPickerComponentProps<'a>,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let (terminal_width, _) = hooks.use_terminal_size();
    let width = if props.width == 0 {
        usize::from(terminal_width)
    } else {
        props.width
    };
    let mut model = hooks.use_state(|| SessionsPickerModel::new(props.request.clone(), width));
    let mut selected_outcome = hooks.use_state(|| Option::<SessionsPickerOutcome>::None);
    let mut should_cancel = hooks.use_state(|| false);

    hooks.use_terminal_events({
        move |event| {
            let TerminalEvent::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) = event
            else {
                return;
            };
            if kind == KeyEventKind::Release {
                return;
            }
            if width < MIN_PICKER_WIDTH {
                return;
            }

            let mut model_value = model.write();
            match code {
                KeyCode::Down => model_value.handle_key(SessionsPickerKey::MoveDown),
                KeyCode::Up => model_value.handle_key(SessionsPickerKey::MoveUp),
                KeyCode::Tab => model_value.handle_key(SessionsPickerKey::CycleRoot),
                KeyCode::BackTab => model_value.handle_key(SessionsPickerKey::CycleProvider),
                KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                    model_value.handle_key(SessionsPickerKey::CycleSource);
                }
                KeyCode::Backspace => model_value.handle_key(SessionsPickerKey::SearchBackspace),
                KeyCode::Char(character) => {
                    model_value.handle_key(SessionsPickerKey::SearchChar(character));
                }
                KeyCode::Enter => selected_outcome.set(model_value.selected_outcome()),
                KeyCode::Esc => should_cancel.set(true),
                _ => {}
            }
        }
    });

    if let Some(selected_outcome) = selected_outcome.read().clone() {
        if let Some(out) = props.selected_outcome_out.as_mut() {
            **out = Some(selected_outcome);
        }
        system.exit();
    } else if *should_cancel.read() {
        system.exit();
    }

    if width < MIN_PICKER_WIDTH {
        if let Some(out) = props.selected_outcome_out.as_mut() {
            **out = Some(SessionsPickerOutcome::TerminalTooNarrow);
        }
        system.exit();
        return element! {
            Text(content: "terminal too narrow\n")
        };
    }

    let snapshot = model.read().render_snapshot();
    element! {
        Text(content: snapshot)
    }
}

pub(crate) fn run_sessions_picker(
    request: SessionsPickerRequest,
) -> io::Result<Option<SessionsPickerOutcome>> {
    let mut selected_outcome = None;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(
        element! {
            SessionsPickerComponent(
                request: request,
                width: 0usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .render_loop(),
    )?;
    Ok(selected_outcome)
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use iocraft::prelude::*;

    use super::*;
    use crate::presentation::session_picker::test_support::picker_request;

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_handles_keys() {
        let mut selected_outcome = None;
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert!(
            actual
                .last()
                .is_some_and(|snapshot| snapshot.contains("Provider migration")),
            "picker should render the selected row before exiting: {actual:?}"
        );
        assert_eq!(
            selected_outcome,
            Some(SessionsPickerOutcome::ResumeSession("thread-b".to_owned()))
        );
    }

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_search_keeps_plain_letters() {
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('r'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('u'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('s'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('t'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert!(
            actual
                .iter()
                .any(|snapshot| snapshot.contains("Search rust")),
            "plain filter letters should search, not switch filters: {actual:?}"
        );
        assert!(
            actual
                .iter()
                .any(|snapshot| snapshot.contains("Root cwd  Provider any  Source interactive")),
            "plain search input should leave filters unchanged: {actual:?}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_too_narrow_exits_without_selection() {
        let mut selected_outcome = None;
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 20usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Enter,
            ))],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            selected_outcome,
            Some(SessionsPickerOutcome::TerminalTooNarrow)
        );
        assert!(
            actual
                .last()
                .is_some_and(|snapshot| snapshot.contains("terminal too narrow")),
            "too-narrow picker should render only the concise error: {actual:?}"
        );
    }
}
