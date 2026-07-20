use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use codex_router_core::ids::AccountId;
use iocraft::prelude::*;

use super::quota_browse_rendering::detail_line;
use super::quota_status_view_model::QuotaStatusViewModel;
use super::quota_status_view_model::QuotaStatusViewModelLoader;

pub(super) const MAX_WEEKLY_FLOOR_PERCENT: u16 = 15;
const WEEKLY_FLOOR_EDITOR_HEIGHT: usize = 10;

pub(crate) type WeeklyQuotaFloorSaver =
    Arc<dyn Fn(AccountId, u16) -> WeeklyQuotaFloorSaveFuture + Send + Sync>;
pub(crate) type WeeklyQuotaFloorSaveFuture =
    Pin<Box<dyn Future<Output = Result<(), WeeklyQuotaFloorSaveError>> + Send>>;
pub(super) type QuotaStatusReloadLock = Arc<tokio::sync::Mutex<()>>;

pub(super) enum WeeklyFloorEditorCommand {
    Save { account_id: AccountId, percent: u16 },
    Reload,
}

#[derive(Clone)]
pub(super) struct WeeklyFloorEditorCommandPort {
    sender: tokio::sync::mpsc::UnboundedSender<WeeklyFloorEditorCommand>,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<WeeklyFloorEditorCommand>>>>,
}

impl WeeklyFloorEditorCommandPort {
    pub(super) fn new() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            sender,
            receiver: Arc::new(Mutex::new(Some(receiver))),
        }
    }

    pub(super) fn send(&self, command: WeeklyFloorEditorCommand) -> bool {
        self.sender.send(command).is_ok()
    }

    pub(super) fn take_receiver(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<WeeklyFloorEditorCommand>> {
        self.receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WeeklyQuotaFloorSaveError {
    DatabaseBusy,
    SchemaUpgradeRequired,
    AccountNotFound,
    StateOperationFailed,
}

impl WeeklyQuotaFloorSaveError {
    pub(super) const fn display_message(self) -> &'static str {
        match self {
            Self::DatabaseBusy => "database busy; retry save",
            Self::SchemaUpgradeRequired => "serve must upgrade the router database",
            Self::AccountNotFound => "account is no longer available",
            Self::StateOperationFailed => "weekly floor update failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WeeklyFloorStep {
    Decrease,
    Increase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WeeklyFloorEditorPhase {
    Editing,
    Saving,
    SaveFailed(WeeklyQuotaFloorSaveError),
    SavedRefreshFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WeeklyFloorEditorState {
    pub(super) account_id: AccountId,
    pub(super) account_label: String,
    pub(super) current_percent: u16,
    pub(super) draft_percent: u16,
    pub(super) phase: WeeklyFloorEditorPhase,
}

impl WeeklyFloorEditorState {
    pub(super) fn new(account_id: AccountId, account_label: String, current_percent: u16) -> Self {
        Self {
            account_id,
            account_label,
            current_percent,
            draft_percent: current_percent.min(MAX_WEEKLY_FLOOR_PERCENT),
            phase: WeeklyFloorEditorPhase::Editing,
        }
    }

    pub(super) const fn can_adjust(&self) -> bool {
        matches!(
            self.phase,
            WeeklyFloorEditorPhase::Editing | WeeklyFloorEditorPhase::SaveFailed(_)
        )
    }
}

pub(super) fn step_weekly_floor_percent(percent: u16, step: WeeklyFloorStep) -> u16 {
    match step {
        WeeklyFloorStep::Decrease => percent.saturating_sub(1),
        WeeklyFloorStep::Increase => percent.saturating_add(1).min(MAX_WEEKLY_FLOOR_PERCENT),
    }
}

pub(super) fn step_weekly_floor_editor(
    state: &mut Option<WeeklyFloorEditorState>,
    step: WeeklyFloorStep,
) {
    let Some(editor) = state.as_mut() else {
        return;
    };
    if !editor.can_adjust() {
        return;
    }
    editor.draft_percent = step_weekly_floor_percent(editor.draft_percent, step);
    editor.phase = WeeklyFloorEditorPhase::Editing;
}

pub(super) fn weekly_floor_editor_key_command(
    state: &mut Option<WeeklyFloorEditorState>,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<WeeklyFloorEditorCommand> {
    let phase = state.as_ref().map(|editor| editor.phase)?;
    let can_cancel = matches!(
        phase,
        WeeklyFloorEditorPhase::Editing | WeeklyFloorEditorPhase::SaveFailed(_)
    );
    match code {
        KeyCode::Left => step_weekly_floor_editor(state, WeeklyFloorStep::Decrease),
        KeyCode::Right => step_weekly_floor_editor(state, WeeklyFloorStep::Increase),
        KeyCode::Enter => {
            let command = match state.as_ref() {
                Some(editor)
                    if matches!(
                        editor.phase,
                        WeeklyFloorEditorPhase::Editing | WeeklyFloorEditorPhase::SaveFailed(_)
                    ) =>
                {
                    Some(WeeklyFloorEditorCommand::Save {
                        account_id: editor.account_id.clone(),
                        percent: editor.draft_percent,
                    })
                }
                Some(editor) if editor.phase == WeeklyFloorEditorPhase::SavedRefreshFailed => {
                    Some(WeeklyFloorEditorCommand::Reload)
                }
                _ => None,
            };
            if command.is_some()
                && let Some(editor) = state.as_mut()
            {
                editor.phase = WeeklyFloorEditorPhase::Saving;
            }
            return command;
        }
        KeyCode::Esc if can_cancel => *state = None,
        KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) && can_cancel => {
            *state = None;
        }
        _ => {}
    }
    None
}

pub(super) fn weekly_floor_editor_send_failed(state: &mut Option<WeeklyFloorEditorState>) {
    if let Some(editor) = state.as_mut() {
        editor.phase =
            WeeklyFloorEditorPhase::SaveFailed(WeeklyQuotaFloorSaveError::StateOperationFailed);
    }
}

pub(super) async fn run_weekly_floor_editor_commands(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<WeeklyFloorEditorCommand>,
    saver: Option<WeeklyQuotaFloorSaver>,
    loader: Option<QuotaStatusViewModelLoader>,
    reload_lock: QuotaStatusReloadLock,
    mut view_model: State<QuotaStatusViewModel>,
    mut editor_state: State<Option<WeeklyFloorEditorState>>,
) {
    while let Some(command) = receiver.recv().await {
        let mut committed_floor = None;
        let save_result = match command {
            WeeklyFloorEditorCommand::Save {
                account_id,
                percent,
            } => match saver.as_ref() {
                Some(saver) => {
                    let result = saver(account_id.clone(), percent).await;
                    if result.is_ok() {
                        committed_floor = Some((account_id, percent));
                    }
                    result
                }
                None => Err(WeeklyQuotaFloorSaveError::StateOperationFailed),
            },
            WeeklyFloorEditorCommand::Reload => Ok(()),
        };
        if let Err(error) = save_result {
            if let Some(editor) = editor_state.write().as_mut() {
                editor.phase = WeeklyFloorEditorPhase::SaveFailed(error);
            }
            continue;
        }
        if let Some((account_id, percent)) = committed_floor
            && let Some(row) = view_model
                .write()
                .rows
                .iter_mut()
                .find(|row| row.account_id == account_id)
        {
            row.weekly_quota_floor_percent = percent;
        }
        let _reload_guard = reload_lock.lock().await;
        let reloaded = match loader.as_ref() {
            Some(loader) => loader().await,
            None => None,
        };
        if let Some(reloaded) = reloaded {
            view_model.set(reloaded);
            editor_state.set(None);
        } else if let Some(editor) = editor_state.write().as_mut() {
            editor.current_percent = editor.draft_percent;
            editor.phase = WeeklyFloorEditorPhase::SavedRefreshFailed;
        }
    }
}

pub(super) const fn weekly_floor_editor_content_height() -> usize {
    WEEKLY_FLOOR_EDITOR_HEIGHT
}

pub(super) fn weekly_floor_editor_footer(editor: &WeeklyFloorEditorState) -> &'static str {
    match editor.phase {
        WeeklyFloorEditorPhase::Editing | WeeklyFloorEditorPhase::SaveFailed(_) => {
            "←/→ adjust  enter save  esc/ctrl-e cancel"
        }
        WeeklyFloorEditorPhase::Saving => "saving weekly floor",
        WeeklyFloorEditorPhase::SavedRefreshFailed => "enter retry refresh  ctrl-c exit",
    }
}

pub(super) fn render_weekly_floor_editor(
    editor: &WeeklyFloorEditorState,
    width: usize,
    height: usize,
    mut editor_state: State<Option<WeeklyFloorEditorState>>,
) -> AnyElement<'static> {
    let detail_width = width.saturating_sub(4).max(24);
    let status = match editor.phase {
        WeeklyFloorEditorPhase::Editing => None,
        WeeklyFloorEditorPhase::Saving => Some(("saving…", Color::Yellow)),
        WeeklyFloorEditorPhase::SaveFailed(error) => Some((error.display_message(), Color::Red)),
        WeeklyFloorEditorPhase::SavedRefreshFailed => {
            Some(("saved; refresh failed", Color::Yellow))
        }
    };
    let current_floor = if editor.current_percent == 0 {
        "disabled (0%)".to_owned()
    } else {
        format!("{}%", editor.current_percent)
    };
    let decrease_editor_state = editor_state;
    let increase_editor_state = editor_state;

    element! {
        View(
            width: width as u32,
            height: height as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
        ) {
            View(width: detail_width as u32, flex_direction: FlexDirection::Column) {
                Text(content: "Weekly floor", color: Color::Cyan, weight: Weight::Bold)
                #(detail_line("account", &editor.account_label, detail_width, Color::White))
                #(detail_line("current", &current_floor, detail_width, Color::White))
                View(height: 1) {}
                View(width: detail_width as u32, align_items: AlignItems::Center) {
                    View(width: 10) {
                        Text(content: "floor", color: Color::Grey, wrap: TextWrap::NoWrap)
                    }
                    Button(
                        handler: move |_| {
                            let mut next = decrease_editor_state.read().clone();
                            step_weekly_floor_editor(&mut next, WeeklyFloorStep::Decrease);
                            editor_state.set(next);
                        },
                        has_focus: false,
                    ) {
                        Text(content: "◀", color: Color::Cyan, weight: Weight::Bold)
                    }
                    View(width: 7, justify_content: JustifyContent::Center) {
                        Text(content: format!("{}%", editor.draft_percent), color: Color::White, weight: Weight::Bold, wrap: TextWrap::NoWrap)
                    }
                    Button(
                        handler: move |_| {
                            let mut next = increase_editor_state.read().clone();
                            step_weekly_floor_editor(&mut next, WeeklyFloorStep::Increase);
                            editor_state.set(next);
                        },
                        has_focus: false,
                    ) {
                        Text(content: "▶", color: Color::Cyan, weight: Weight::Bold)
                    }
                }
                Text(content: "0% disables protection · maximum 15%", color: Color::Grey)
                #(status.map(|(message, color)| element! { Text(content: message, color) }.into_any()))
            }
        }
    }
    .into_any()
}
