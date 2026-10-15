//! Quota-owned contextual Help and surface chrome; no command or runtime ownership.
use codex_router_tui::{
    DialogAction, DialogActionEmphasis, ShortcutEntry, ShortcutHelp, ShortcutSection,
    SurfaceHeader, UiText, UiTextRole,
};
use iocraft::prelude::*;

use super::quota_status_view_model::QuotaStatusViewModel;
use super::responsive_quota_layout::{
    quota_spinner_tick, quota_title_row, quota_title_status, truncate_quota_header_text,
};

#[derive(Clone, Copy, Default)]
pub(super) enum QuotaHelpMode {
    #[default]
    Browse,
    Filters,
    Details,
    Floor,
    Options,
    Reset,
}

pub(super) fn quota_help_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::F(1) | KeyCode::Char('\u{1f}'))
        || (matches!(code, KeyCode::Char('/' | '_' | '7'))
            && modifiers.contains(KeyModifiers::CONTROL))
}

pub(super) fn quota_help_action(mut open: State<bool>) -> AnyElement<'static> {
    element!(DialogAction(label: "Help".to_owned(), emphasis: DialogActionEmphasis::Secondary, handler: move |_| open.set(true))).into_any()
}

pub(super) struct QuotaHeaderProps<'a> {
    pub model: &'a QuotaStatusViewModel,
    pub width: usize,
    pub tick: usize,
    pub interactive: bool,
    pub inactive: bool,
    pub help: State<bool>,
    pub details_action: AnyElement<'static>,
}

pub(super) fn render_quota_header(props: QuotaHeaderProps<'_>) -> AnyElement<'static> {
    let QuotaHeaderProps {
        model,
        width,
        tick,
        interactive,
        inactive,
        help,
        details_action,
    } = props;
    if !interactive {
        return quota_title_row(model, width, tick);
    }
    let column_gap: u16 = if width < 64 { 1 } else { 2 };
    // Keep the title and both actions; reserve their actual labels and three gaps.
    let status_width = width.saturating_sub(23 + 3 * usize::from(column_gap));
    let full_status = quota_title_status(model, tick);
    let status = if width < 64
        && unicode_width::UnicodeWidthStr::width(full_status.as_str()) > status_width
    {
        // Operational words take precedence over a decorative spinner on phones.
        full_status
            .strip_prefix(quota_spinner_tick(tick))
            .unwrap_or(&full_status)
            .trim_start()
    } else {
        full_status.as_str()
    };
    let status = truncate_quota_header_text(status, status_width);
    element!(SurfaceHeader(title: "Quota status".to_owned(), inactive, column_gap, help_action: Some(quota_help_action(help)), children: vec![element!(UiText(content: status, role: UiTextRole::Secondary)).into_any(), details_action])).into_any()
}

pub(super) fn render_quota_help(
    width: u16,
    height: u16,
    mode: QuotaHelpMode,
    mut open: State<bool>,
) -> AnyElement<'static> {
    let (title, entries) = match mode {
        QuotaHelpMode::Browse => (
            "Browse accounts",
            vec![
                ("↑ / ↓", "Focus account"),
                ("Ctrl+F / F3", "Open filters"),
                ("F4", "Account details"),
                ("Ctrl+E", "Edit weekly floor"),
                ("Ctrl+R", "Account options"),
                ("Esc / Q", "Exit"),
            ],
        ),
        QuotaHelpMode::Filters => (
            "Account filters",
            vec![
                ("↑ / ↓", "Choose provider"),
                ("Tab / Shift+Tab", "Move focus"),
                ("Space", "Toggle disabled accounts"),
                ("Enter", "Apply draft; Cancel when focused"),
                ("Esc", "Discard draft"),
            ],
        ),
        QuotaHelpMode::Details => (
            "Account details",
            vec![
                ("↑ / ↓ / PgUp / PgDn", "Scroll details"),
                ("Esc", "Close details"),
            ],
        ),
        QuotaHelpMode::Floor => (
            "Weekly floor",
            vec![
                ("← / →", "Adjust floor"),
                ("Enter", "Save or retry refresh"),
                ("Esc / Ctrl+E", "Cancel uncommitted edit"),
            ],
        ),
        QuotaHelpMode::Options => (
            "Account options",
            vec![
                ("Tab / ← / →", "Switch Resets and Credits"),
                ("Enter", "Inspect resets or edit/save credit preference"),
                ("R", "Refresh credit usage"),
                ("Esc / Ctrl+R", "Back"),
            ],
        ),
        QuotaHelpMode::Reset => (
            "Reset workflow",
            vec![
                ("PgUp / PgDn", "Page reset inventory"),
                ("Enter", "Review or confirm selected action"),
                ("← / →", "Select No or enabled Yes"),
                ("Esc", "Cancel before commitment"),
            ],
        ),
    };
    let sections = vec![
        ShortcutSection {
            title: title.to_owned(),
            entries: entries
                .into_iter()
                .map(|(keys, description)| ShortcutEntry {
                    keys: keys.to_owned(),
                    description: description.to_owned(),
                })
                .collect(),
        },
        ShortcutSection {
            title: "Help".to_owned(),
            entries: vec![
                ShortcutEntry {
                    keys: "F1 / Ctrl+/".to_owned(),
                    description: "Open Help; F1 or Esc closes Help".to_owned(),
                },
                ShortcutEntry {
                    keys: "Ctrl+C / Ctrl+D".to_owned(),
                    description: "Existing forced-exit rules still apply".to_owned(),
                },
            ],
        },
    ];
    element!(ShortcutHelp(width, height, title: "Quota Help".to_owned(), sections, actions: vec![element!(DialogAction(label: "Close".to_owned(), handler: move |_| open.set(false))).into_any()])).into_any()
}

pub(super) struct QuotaScreenChromeProps {
    pub width: usize,
    pub height: usize,
    pub header: AnyElement<'static>,
    pub route: String,
    pub controls: Vec<AnyElement<'static>>,
    pub body: AnyElement<'static>,
    pub actions: Vec<AnyElement<'static>>,
    pub static_footer: Option<String>,
}

pub(super) fn render_quota_screen(props: QuotaScreenChromeProps) -> AnyElement<'static> {
    element! {
        View(width: props.width as u32, height: props.height as u32, border_style: BorderStyle::Round, border_color: Color::Cyan, overflow: Overflow::Hidden, padding_left: 1, padding_right: 1, flex_direction: FlexDirection::Column) {
            #(props.header)
            Text(content: props.route, color: Color::White, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            #(props.controls)
            #(props.body)
            View(width: 100pct, flex_grow: 1.0_f32) {}
            View(flex_shrink: 0.0_f32) { #(props.actions) }
            #(props.static_footer.map(|line| element!(Text(content: line, color: Color::Grey, wrap: TextWrap::NoWrap)).into_any()))
        }
    }.into_any()
}

// Legacy static output keeps its current workflow footer; interactive Help owns
// shortcut presentation instead. This function has no input or effect handling.
pub(super) struct QuotaFooterPresentation<'a> {
    pub weekly_floor_editor: Option<&'a super::quota_floor_editor::WeeklyFloorEditorState>,
    pub account_options: Option<&'a super::quota_account_options::AccountOptionsState>,
    pub reset_snapshot:
        Option<&'a crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot>,
    pub width: usize,
    pub weekly_floor_editing_enabled: bool,
    pub reset_detail_active: bool,
    pub filters: super::quota_provider_filters::QuotaFilterState,
    pub filter_modal_owns_input: bool,
}

pub(super) fn quota_legacy_footer(props: QuotaFooterPresentation<'_>) -> String {
    use super::quota_account_options::account_options_footer;
    use super::quota_floor_editor::weekly_floor_editor_footer;
    use super::quota_reset_detail_rendering::reset_footer;
    let inspection_footer = props
        .account_options
        .and(props.reset_snapshot)
        .and_then(|snapshot| {
            if props
                .account_options
                .is_some_and(|options| options.pending_tab.is_some())
                && matches!(
                    snapshot.phase(),
                    crate::quota_reset::reset_session_supervisor::WorkflowPhase::Inspecting
                        | crate::quota_reset::reset_session_supervisor::WorkflowPhase::Inspected
                )
            {
                Some("tab change pending  esc back  ctrl-c exit")
            } else {
                super::quota_account_options::account_options_inspection_footer(
                    snapshot.phase(),
                    props.width,
                )
            }
        });
    let mut footer_line = props.weekly_floor_editor.map_or_else(
        || {
            props.account_options.map_or_else(
                || {
                    if props.weekly_floor_editing_enabled && !props.reset_detail_active {
                        "↑/↓ focus  ctrl-e edit floor  ctrl-r account options  esc/q exit  ctrl-c exit".to_owned()
                    } else {
                        reset_footer(props.reset_snapshot).to_owned()
                    }
                },
                |options| {
                    if props.reset_detail_active {
                        inspection_footer.map_or_else(
                            || reset_footer(props.reset_snapshot).to_owned(),
                            str::to_owned,
                        )
                    } else {
                        account_options_footer(options)
                    }
                },
            )
        },
        |editor| weekly_floor_editor_footer(editor).to_owned(),
    );
    if props.filters.draft.is_some() && !props.filter_modal_owns_input {
        footer_line = "↑/↓ provider  tab focus  space toggle  enter apply  esc cancel".to_owned();
    }
    footer_line
}
