//! Iocraft presentation for the interactive quota-reset workflow.

use std::io;

use iocraft::prelude::*;

use crate::quota_reset::credentials::ResetAccountChoice;
use crate::quota_reset::orchestration::PreparedReset;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResetConfirmationOutcome {
    Cancel,
    Confirm,
}

pub(crate) async fn select_reset_account(
    accounts: Vec<ResetAccountChoice>,
) -> io::Result<Option<ResetAccountChoice>> {
    let mut selected_account = None;
    element! {
        QuotaResetAccountPicker(
            accounts,
            selected_account_out: &mut selected_account,
            width: 0usize,
        )
    }
    .render_loop()
    .ignore_ctrl_c()
    .await?;
    Ok(selected_account)
}

pub(crate) async fn confirm_prepared_reset(
    accounts: Vec<ResetAccountChoice>,
    account: ResetAccountChoice,
    prepared: PreparedReset,
) -> io::Result<ResetConfirmationOutcome> {
    let mut outcome = ResetConfirmationOutcome::Cancel;
    element! {
        QuotaResetConfirmation(
            accounts,
            account,
            prepared,
            outcome_out: &mut outcome,
            width: 0usize,
        )
    }
    .render_loop()
    .ignore_ctrl_c()
    .await?;
    Ok(outcome)
}

#[derive(Default, Props)]
struct QuotaResetAccountPickerProps<'a> {
    accounts: Vec<ResetAccountChoice>,
    selected_account_out: Option<&'a mut Option<ResetAccountChoice>>,
    width: usize,
}

#[component]
fn QuotaResetAccountPicker<'a>(
    props: &mut QuotaResetAccountPickerProps<'a>,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let (terminal_width, _) = hooks.use_terminal_size();
    let selected_index = hooks.use_state(|| 0usize);
    let mut outcome = hooks.use_state(|| Option::<ResetAccountChoice>::None);
    let mut cancelled = hooks.use_state(|| false);
    let account_count = props.accounts.len();
    let accounts_for_events = props.accounts.clone();
    hooks.use_terminal_events({
        let mut selected_index = selected_index;
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
            match code {
                KeyCode::Up => selected_index.set(selected_index.get().saturating_sub(1)),
                KeyCode::Down if account_count > 0 => {
                    selected_index.set((selected_index.get() + 1).min(account_count - 1));
                }
                KeyCode::Enter if account_count > 0 => {
                    if let Some(account) = accounts_for_events.get(selected_index.get()) {
                        outcome.set(Some(account.clone()));
                    }
                }
                KeyCode::Esc => cancelled.set(true),
                KeyCode::Char('c' | 'r') if modifiers.contains(KeyModifiers::CONTROL) => {
                    cancelled.set(true);
                }
                KeyCode::Char('\u{3}') => cancelled.set(true),
                _ => {}
            }
        }
    });
    if *cancelled.read() || outcome.read().is_some() {
        if let Some(output) = props.selected_account_out.as_mut() {
            **output = outcome.read().clone();
        }
        system.exit();
    }

    let width = if props.width == 0 {
        usize::from(terminal_width).max(48)
    } else {
        props.width.max(48)
    };
    let selected_index = selected_index.get();
    let account_rows = props
        .accounts
        .iter()
        .enumerate()
        .map(|(index, account)| {
            let marker = if index == selected_index { "❯" } else { " " };
            let color = if index == selected_index {
                Color::Yellow
            } else {
                Color::White
            };
            element! {
                Text(
                    content: fit_line(&format!("{marker}  {}  [{}]", account.label, account.account_tag), width.saturating_sub(6)),
                    color,
                    weight: if index == selected_index { Weight::Bold } else { Weight::Normal },
                    wrap: TextWrap::NoWrap,
                )
            }
        })
        .collect::<Vec<_>>();
    let selected_label = props
        .accounts
        .get(selected_index)
        .map_or("No account available", |account| account.label.as_str());

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: Color::Cyan,
            padding: 1,
        ) {
            Text(content: "Quota reset", color: Color::Cyan, weight: Weight::Bold)
            View(border_style: BorderStyle::Single, border_color: Color::DarkGrey, padding: 1) {
                #(account_rows)
            }
            View(border_style: BorderStyle::Single, border_color: Color::DarkGrey, padding: 1) {
                Text(content: "Selected account", color: Color::Cyan, weight: Weight::Bold)
                Text(content: selected_label, color: Color::White, weight: Weight::Bold)
                Text(content: "Enter checks live weekly usage. Saved quota is never used.", color: Color::Grey)
            }
            Text(content: "↑/↓ select  enter check live quota  esc exit  ctrl-c exit  ctrl-r exit", color: Color::Grey)
        }
    }
}

#[derive(Default, Props)]
struct QuotaResetConfirmationProps<'a> {
    accounts: Vec<ResetAccountChoice>,
    account: Option<ResetAccountChoice>,
    prepared: Option<PreparedReset>,
    outcome_out: Option<&'a mut ResetConfirmationOutcome>,
    width: usize,
}

#[component]
fn QuotaResetConfirmation<'a>(
    props: &mut QuotaResetConfirmationProps<'a>,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let (terminal_width, _) = hooks.use_terminal_size();
    let selected_yes = hooks.use_state(|| false);
    let mut finished = hooks.use_state(|| false);
    let mut cancelled = hooks.use_state(|| false);
    hooks.use_terminal_events({
        let mut selected_yes = selected_yes;
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
            match code {
                KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
                    selected_yes.set(!selected_yes.get());
                }
                KeyCode::Enter => finished.set(true),
                KeyCode::Esc => cancelled.set(true),
                KeyCode::Char('c' | 'r') if modifiers.contains(KeyModifiers::CONTROL) => {
                    cancelled.set(true);
                }
                KeyCode::Char('\u{3}') => cancelled.set(true),
                _ => {}
            }
        }
    });
    if *cancelled.read() || *finished.read() {
        let outcome = if *finished.read() && selected_yes.get() {
            ResetConfirmationOutcome::Confirm
        } else {
            ResetConfirmationOutcome::Cancel
        };
        if let Some(output) = props.outcome_out.as_mut() {
            **output = outcome;
        }
        system.exit();
    }

    let width = if props.width == 0 {
        usize::from(terminal_width).max(48)
    } else {
        props.width.max(48)
    };
    let no_marker = if selected_yes.get() { " " } else { "❯" };
    let yes_marker = if selected_yes.get() { "❯" } else { " " };
    let expiry = props
        .prepared
        .as_ref()
        .and_then(|prepared| prepared.expires_at.clone())
        .unwrap_or_else(|| "does not expire".to_owned());
    let account_label = props
        .account
        .as_ref()
        .map_or("unknown", |account| account.label.as_str());
    let account_tag = props
        .account
        .as_ref()
        .map_or("unknown", |account| account.account_tag.as_str());
    let weekly_remaining_percent = props
        .prepared
        .as_ref()
        .map_or(0, |prepared| prepared.weekly_remaining_percent);
    let reset_title = props
        .prepared
        .as_ref()
        .and_then(|prepared| prepared.credit_title.as_deref())
        .unwrap_or("Usage-limit reset");
    let credit_hint = props
        .prepared
        .as_ref()
        .map_or("unknown", |prepared| credit_id_hint(&prepared.credit_id));
    let selected_account_id = props.account.as_ref().map(|account| &account.account_id);
    let account_rows = props
        .accounts
        .iter()
        .map(|account| {
            let selected = selected_account_id == Some(&account.account_id);
            let marker = if selected { "❯" } else { " " };
            element! {
                Text(
                    content: fit_line(&format!("{marker}  {}  [{}]", account.label, account.account_tag), width.saturating_sub(6)),
                    color: if selected { Color::Yellow } else { Color::White },
                    weight: if selected { Weight::Bold } else { Weight::Normal },
                    wrap: TextWrap::NoWrap,
                )
            }
        })
        .collect::<Vec<_>>();

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: Color::Cyan,
            padding: 1,
        ) {
            Text(content: "Confirm usage-limit reset", color: Color::Cyan, weight: Weight::Bold)
            View(border_style: BorderStyle::Single, border_color: Color::DarkGrey, padding: 1) {
                #(account_rows)
            }
            Text(content: format!("Account           {account_label}  [{account_tag}]"), color: Color::White)
            Text(content: format!("Weekly remaining  {weekly_remaining_percent}% (live)"), color: Color::White)
            Text(content: format!("Selected reset    {reset_title}"), color: Color::White)
            Text(content: format!("Credit ID         {credit_hint}"), color: Color::Grey)
            Text(content: format!("Credit expires    {expiry}"), color: Color::White)
            Text(content: "This consumes one reset credit.", color: Color::Red, weight: Weight::Bold)
            Text(content: format!("{no_marker}  No, cancel"), color: if selected_yes.get() { Color::Grey } else { Color::Yellow }, weight: Weight::Bold)
            Text(content: format!("{yes_marker}  Yes, use this reset"), color: if selected_yes.get() { Color::Yellow } else { Color::Grey }, weight: Weight::Bold)
            Text(content: "↑/↓ select  enter confirm  esc exit  ctrl-c exit  ctrl-r exit", color: Color::Grey)
        }
    }
}

fn fit_line(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    if width <= 1 {
        return "…".to_owned();
    }
    format!("{}…", value.chars().take(width - 1).collect::<String>())
}

fn credit_id_hint(credit_id: &str) -> &str {
    if credit_id.chars().count() <= 6 {
        return "[redacted]";
    }
    let start = credit_id
        .char_indices()
        .rev()
        .nth(5)
        .map_or(0, |(index, _)| index);
    credit_id.get(start..).unwrap_or("redacted")
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;
    use codex_router_core::ids::AccountId;

    #[tokio::test]
    async fn confirmation_defaults_to_no_and_enter_cancels() {
        let mut outcome = ResetConfirmationOutcome::Confirm;
        let frames = element! {
            QuotaResetConfirmation(
                accounts: vec![account("primary")],
                account: account("primary"),
                prepared: prepared(),
                outcome_out: &mut outcome,
                width: 80usize,
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

        assert!(frames.iter().any(|frame| frame.contains("❯  No, cancel")));
        assert_eq!(outcome, ResetConfirmationOutcome::Cancel);
    }

    #[tokio::test]
    async fn confirmation_requires_selecting_yes_before_enter() {
        let mut outcome = ResetConfirmationOutcome::Cancel;
        let _frames = element! {
            QuotaResetConfirmation(
                accounts: vec![account("primary")],
                account: account("primary"),
                prepared: prepared(),
                outcome_out: &mut outcome,
                width: 80usize,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
            ],
        )))
        .collect::<Vec<_>>()
        .await;

        assert_eq!(outcome, ResetConfirmationOutcome::Confirm);
    }

    #[tokio::test]
    async fn confirmation_cancel_keys_never_confirm() {
        for (label, event) in [
            (
                "escape",
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ),
            ("ctrl-c", ctrl_key('c')),
            ("ctrl-r", ctrl_key('r')),
        ] {
            let mut outcome = ResetConfirmationOutcome::Cancel;
            let _frames = element! {
                QuotaResetConfirmation(
                    accounts: vec![account("primary")],
                    account: account("primary"),
                    prepared: prepared(),
                    outcome_out: &mut outcome,
                    width: 80usize,
                )
            }
            .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
                vec![event],
            )))
            .collect::<Vec<_>>()
            .await;

            assert_eq!(outcome, ResetConfirmationOutcome::Cancel, "{label}");
        }
    }

    #[tokio::test]
    async fn account_picker_ctrl_r_exits_without_selection() {
        let mut selected_account: Option<ResetAccountChoice> = Some(account("sentinel"));
        let _frames = element! {
            QuotaResetAccountPicker(
                accounts: vec![account("primary")],
                selected_account_out: &mut selected_account,
                width: 80usize,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![ctrl_key('r')],
        )))
        .collect::<Vec<_>>()
        .await;

        assert_eq!(selected_account, None);
    }

    #[tokio::test]
    async fn account_picker_enter_returns_highlighted_account() {
        let mut selected_account: Option<ResetAccountChoice> = None;
        let _frames = element! {
            QuotaResetAccountPicker(
                accounts: vec![account("primary"), account("secondary")],
                selected_account_out: &mut selected_account,
                width: 80usize,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
            ],
        )))
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            selected_account.map(|account| account.label),
            Some("secondary".to_owned())
        );
    }

    #[tokio::test]
    async fn confirmation_distinguishes_duplicate_labels_and_preserves_selection() {
        let first = account_with_id("acct_first", "shared");
        let selected = account_with_id("acct_second", "shared");
        let first_tag = first.account_tag.clone();
        let selected_tag = selected.account_tag.clone();
        let mut outcome = ResetConfirmationOutcome::Confirm;
        let frames = element! {
            QuotaResetConfirmation(
                accounts: vec![first, selected.clone()],
                account: selected,
                prepared: prepared(),
                outcome_out: &mut outcome,
                width: 80usize,
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
        let rendered = frames.join("\n");

        assert_ne!(first_tag, selected_tag);
        assert!(rendered.contains(&format!("shared  [{first_tag}]")));
        assert!(rendered.contains(&format!("❯  shared  [{selected_tag}]")));
        assert_eq!(outcome, ResetConfirmationOutcome::Cancel);
    }

    fn account(label: &str) -> ResetAccountChoice {
        account_with_id(&format!("acct_{label}"), label)
    }

    fn account_with_id(account_id: &str, label: &str) -> ResetAccountChoice {
        ResetAccountChoice::for_test(
            AccountId::new(account_id)
                .unwrap_or_else(|error| panic!("account id should parse: {error}")),
            label,
            1,
        )
    }

    fn ctrl_key(character: char) -> TerminalEvent {
        let mut event = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(character));
        event.modifiers = KeyModifiers::CONTROL;
        TerminalEvent::Key(event)
    }

    #[test]
    fn credit_id_hint_never_returns_a_complete_short_identifier() {
        assert_eq!(credit_id_hint("a"), "[redacted]");
        assert_eq!(credit_id_hint("abcdef"), "[redacted]");
        assert_eq!(credit_id_hint("prefix-suffix"), "suffix");
    }

    fn prepared() -> PreparedReset {
        PreparedReset::for_test("credit-a", Some(100))
    }
}
