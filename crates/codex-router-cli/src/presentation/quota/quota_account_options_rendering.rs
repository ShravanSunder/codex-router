use iocraft::prelude::*;

use crate::quota::CreditUsageFreshness;
use crate::quota::CreditUsageStatus;
use crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot;
use crate::quota_reset::reset_session_supervisor::WorkflowPhase;
use codex_router_core::credit_usage::CreditAvailability;

use super::super::quota_reset_detail_rendering::render_reset_panel_content;
use super::super::quota_reset_detail_rendering::reset_options_content_height;
use super::super::quota_reset_presentation_model::ResetPaneTarget;
use super::super::quota_reset_presentation_model::reset_inventory_page_size;
use super::AccountOptionsMessage;
use super::AccountOptionsState;
use super::AccountOptionsTab;
use super::CreditPolicyEditorPhase;

pub(in crate::presentation::quota) struct AccountOptionsPanelProps<'a> {
    pub(in crate::presentation::quota) options: &'a AccountOptionsState,
    pub(in crate::presentation::quota) reset_snapshot: Option<&'a ResetWorkflowSnapshot>,
    pub(in crate::presentation::quota) reset_target: Option<&'a ResetPaneTarget>,
    pub(in crate::presentation::quota) width: usize,
    pub(in crate::presentation::quota) height: usize,
    pub(in crate::presentation::quota) inventory_page_start: usize,
    pub(in crate::presentation::quota) spinner_tick: usize,
}

pub(in crate::presentation::quota) fn render_account_options_panel(
    props: AccountOptionsPanelProps<'_>,
) -> AnyElement<'static> {
    let width = props.width.max(32);
    let height = props.height.max(1);
    let body_height = height.saturating_sub(7);
    let body = match props.options.tab {
        AccountOptionsTab::Resets => render_resets_tab_body(
            props.options,
            props.reset_snapshot,
            props.reset_target,
            width.saturating_sub(2),
            body_height,
            props.inventory_page_start,
            props.spinner_tick,
        ),
        AccountOptionsTab::Credits => {
            render_credit_tab_body(props.options, width.saturating_sub(2), body_height)
        }
    };
    let target = &props.options.target;
    let identity = format!("{}  [{}]", target.account_label, target.account_tag);
    let resets_tab = render_tab_label("Resets", props.options.tab == AccountOptionsTab::Resets);
    let credits_tab = render_tab_label("Credits", props.options.tab == AccountOptionsTab::Credits);

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
            Text(content: truncate_account_options_line(&identity, width.saturating_sub(4)), color: Color::White, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            View(height: 1) {}
            View(width: 100pct, flex_direction: FlexDirection::Row, column_gap: 2) {
                #(resets_tab)
                #(credits_tab)
            }
            View(width: 100pct, height: 1, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {}
            View(height: 1) {}
            #(body)
        }
    }
    .into_any()
}

pub(in crate::presentation::quota) fn account_options_content_height(
    options: &AccountOptionsState,
    reset_snapshot: Option<&ResetWorkflowSnapshot>,
    reset_target: Option<&ResetPaneTarget>,
    inventory_page_start: usize,
) -> usize {
    if options.tab == AccountOptionsTab::Resets
        && let (Some(snapshot), Some(target)) = (reset_snapshot, reset_target)
        && snapshot.phase() != WorkflowPhase::Browse
    {
        return reset_options_content_height(snapshot, target, inventory_page_start)
            .saturating_add(7);
    }
    const OPTIONS_HEADER_ROWS: usize = 7;
    match options.tab {
        AccountOptionsTab::Resets => {
            OPTIONS_HEADER_ROWS + 3 + usize::from(options.message.is_some())
        }
        AccountOptionsTab::Credits => {
            let base_body_rows = 9;
            let floor_row = usize::from(options.target.weekly_quota_floor_percent > 0);
            let provider_control_row =
                usize::from(provider_control_note(&options.target.credit_usage).is_some());
            let editor_status_row =
                usize::from(options.editor.as_ref().is_some_and(|editor| {
                    !matches!(editor.phase, CreditPolicyEditorPhase::Editing)
                }));
            OPTIONS_HEADER_ROWS
                + base_body_rows
                + floor_row
                + provider_control_row
                + editor_status_row
                + usize::from(options.message.is_some())
        }
    }
}

pub(in crate::presentation::quota) fn account_options_reset_inventory_page_size(
    body_height: usize,
) -> usize {
    // The options body omits the detail title, account line and detail-panel borders.
    const OMITTED_RESET_DETAIL_ROWS: usize = 4;
    reset_inventory_page_size(body_height.saturating_add(OMITTED_RESET_DETAIL_ROWS))
}

pub(super) fn account_options_message_text(message: AccountOptionsMessage) -> &'static str {
    match message {
        AccountOptionsMessage::Refreshing => "Refreshing credit balance…",
        AccountOptionsMessage::RefreshFailed => {
            "Credit refresh failed; cached observation retained."
        }
        AccountOptionsMessage::RefreshUnavailable(reason) => reason.message(),
        AccountOptionsMessage::Refreshed => "Credit balance refreshed.",
        AccountOptionsMessage::ResetReviewActive => {
            "Finish or cancel the reset review before switching tabs."
        }
    }
}

pub(in crate::presentation::quota) fn account_options_inspection_footer(
    phase: WorkflowPhase,
    width: usize,
) -> Option<&'static str> {
    const COMPACT_FOOTER_WIDTH: usize = 72;
    match phase {
        WorkflowPhase::Inspecting if width < COMPACT_FOOTER_WIDTH => {
            Some("tab/←/→ credits  esc/ctrl-r back")
        }
        WorkflowPhase::Inspecting => {
            Some("tab/←/→ credits  esc/ctrl-r back  ctrl-c exit without consume")
        }
        WorkflowPhase::Inspected if width < COMPACT_FOOTER_WIDTH => {
            Some("tab/←/→ credits enter review esc/ctrl-r back")
        }
        WorkflowPhase::Inspected => Some(
            "tab/←/→ credits  enter review  pgup/pgdn pages  esc/ctrl-r back  ctrl-c exit without consume",
        ),
        WorkflowPhase::Browse
        | WorkflowPhase::Confirming
        | WorkflowPhase::Revalidating
        | WorkflowPhase::Committing
        | WorkflowPhase::Result => None,
    }
}

pub(super) fn render_credit_usage_body(
    usage: &CreditUsageStatus,
    weekly_quota_floor_percent: u16,
    width: usize,
    height: usize,
    editor: Option<&super::CreditPolicyEditorState>,
    message: Option<AccountOptionsMessage>,
) -> AnyElement<'static> {
    let text_width = width.saturating_sub(2);
    let entitlement = credit_entitlement_label(usage);
    let freshness = credit_freshness_label(usage.freshness);
    let age = if usage.age_label == "unknown" {
        "not checked".to_owned()
    } else {
        format!("checked {} ago", usage.age_label)
    };
    let policy = match editor {
        Some(editor) => editor.draft_policy,
        None => usage.policy,
    };
    let is_editing = editor.is_some_and(|editor| {
        matches!(
            editor.phase,
            CreditPolicyEditorPhase::Editing | CreditPolicyEditorPhase::SaveFailed(_)
        )
    });
    let balance_heading = element! {
        Text(content: "Credit balance", color: Color::Cyan, weight: Weight::Bold, wrap: TextWrap::NoWrap)
    }
    .into_any();
    let usage_heading = element! {
        Text(content: "Credit usage", color: Color::Cyan, weight: Weight::Bold, wrap: TextWrap::NoWrap)
    }
    .into_any();
    let policy_row = render_policy_row(policy, is_editing, width);
    let provider_note = provider_control_note(usage);
    let floor_note = (weekly_quota_floor_percent > 0)
        .then(|| format!("Weekly floor {weekly_quota_floor_percent}% blocks credit routing."));
    let editor_status = editor.and_then(|editor| match editor.phase {
        CreditPolicyEditorPhase::Editing => None,
        CreditPolicyEditorPhase::Saving => Some(("Saving preference…", Color::Yellow)),
        CreditPolicyEditorPhase::SaveFailed(error) => Some((error.message(), Color::Red)),
        CreditPolicyEditorPhase::SavedReloadFailed => Some((
            "Saved, but account data could not be reloaded.",
            Color::Yellow,
        )),
    });
    let message = message.map(account_options_message_text);

    element! {
        View(width: width as u32, height: height as u32, flex_direction: FlexDirection::Column, overflow: Overflow::Hidden) {
            #(balance_heading)
            #(account_options_field_line("Status", &entitlement, width, Color::White))
            #(account_options_field_line("Freshness", freshness, width, freshness_color(usage.freshness)))
            #(account_options_field_line("Observation", &age, width, Color::Grey))
            View(height: 1) {}
            #(usage_heading)
            #(policy_row)
            Text(content: truncate_account_options_line("Included quota on eligible accounts always comes first.", text_width), color: Color::Grey, wrap: TextWrap::NoWrap)
            Text(content: truncate_account_options_line("Allow is limited to eligible OpenAI Responses requests.", text_width), color: Color::Grey, wrap: TextWrap::NoWrap)
            #(floor_note.map(|note| element! { Text(content: truncate_account_options_line(&note, text_width), color: Color::Yellow, wrap: TextWrap::NoWrap) }.into_any()))
            #(provider_note.map(|note| element! { Text(content: truncate_account_options_line(&note, text_width), color: Color::Yellow, wrap: TextWrap::NoWrap) }.into_any()))
            #(editor_status.map(|(text, color)| element! { Text(content: truncate_account_options_line(text, text_width), color, wrap: TextWrap::NoWrap) }.into_any()))
            #(message.map(|text| element! { Text(content: truncate_account_options_line(text, text_width), color: Color::Yellow, wrap: TextWrap::NoWrap) }.into_any()))
        }
    }
    .into_any()
}

fn render_credit_tab_body(
    options: &AccountOptionsState,
    width: usize,
    height: usize,
) -> AnyElement<'static> {
    render_credit_usage_body(
        &options.target.credit_usage,
        options.target.weekly_quota_floor_percent,
        width,
        height,
        options.editor.as_ref(),
        options.message,
    )
}

fn render_resets_tab_body(
    options: &AccountOptionsState,
    reset_snapshot: Option<&ResetWorkflowSnapshot>,
    reset_target: Option<&ResetPaneTarget>,
    width: usize,
    height: usize,
    inventory_page_start: usize,
    spinner_tick: usize,
) -> AnyElement<'static> {
    if let (Some(snapshot), Some(target)) = (reset_snapshot, reset_target)
        && snapshot.phase() != WorkflowPhase::Browse
    {
        return render_reset_panel_content(
            snapshot,
            target,
            width,
            height,
            inventory_page_start,
            account_options_reset_inventory_page_size(height),
            spinner_tick,
        );
    }
    let unavailable_reason = if !options.target.enabled {
        Some("Resets are unavailable while this account is disabled.")
    } else if options.target.credential_generation.is_none() {
        Some("Resets are unavailable without active credentials.")
    } else {
        None
    };
    let explanation = if let Some(reason) = unavailable_reason {
        reason
    } else if reset_target.is_some() {
        "Checking reset eligibility…"
    } else {
        "Press Enter to inspect reset eligibility."
    };
    let text_width = width.saturating_sub(2);
    element! {
        View(width: width as u32, height: height as u32, flex_direction: FlexDirection::Column) {
            Text(content: "Reset credits", color: Color::Cyan, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            Text(content: truncate_account_options_line(explanation, text_width), color: Color::Grey, wrap: TextWrap::NoWrap)
            Text(content: truncate_account_options_line("No reset is consumed without explicit confirmation.", text_width), color: Color::Grey, wrap: TextWrap::NoWrap)
            #(options.message.map(|message| element! { Text(content: truncate_account_options_line(account_options_message_text(message), text_width), color: Color::Yellow, wrap: TextWrap::NoWrap) }.into_any()))
        }
    }
    .into_any()
}

fn render_tab_label(label: &str, active: bool) -> AnyElement<'static> {
    let (content, color, weight) = if active {
        (format!("[ {label} ]"), Color::Yellow, Weight::Bold)
    } else {
        (label.to_owned(), Color::Grey, Weight::Normal)
    };
    element! { Text(content, color, weight, wrap: TextWrap::NoWrap) }.into_any()
}

fn render_policy_row(
    policy: codex_router_core::credit_usage::CreditUsagePolicy,
    editing: bool,
    width: usize,
) -> AnyElement<'static> {
    if !editing {
        let label = match policy {
            codex_router_core::credit_usage::CreditUsagePolicy::Allow => "Allow",
            codex_router_core::credit_usage::CreditUsagePolicy::Disallow => "Disallow",
        };
        return account_options_field_line("Usage", label, width, Color::Yellow);
    }
    let allow_active = policy.allows_credit_usage();
    let disallow_active = !allow_active;
    element! {
        View(width: width as u32, flex_direction: FlexDirection::Row, column_gap: 2) {
            View(width: 14) { Text(content: "Usage", color: Color::Grey, wrap: TextWrap::NoWrap) }
            View(flex_direction: FlexDirection::Row, column_gap: 1) {
                Text(content: if disallow_active { "›" } else { " " }, color: if disallow_active { Color::Yellow } else { Color::Grey }, weight: Weight::Bold, wrap: TextWrap::NoWrap)
                Text(content: "Disallow", color: if disallow_active { Color::Yellow } else { Color::Grey }, weight: if disallow_active { Weight::Bold } else { Weight::Normal }, wrap: TextWrap::NoWrap)
            }
            View(flex_direction: FlexDirection::Row, column_gap: 1) {
                Text(content: if allow_active { "›" } else { " " }, color: if allow_active { Color::Yellow } else { Color::Grey }, weight: Weight::Bold, wrap: TextWrap::NoWrap)
                Text(content: "Allow", color: if allow_active { Color::Yellow } else { Color::Grey }, weight: if allow_active { Weight::Bold } else { Weight::Normal }, wrap: TextWrap::NoWrap)
            }
        }
    }
    .into_any()
}

fn account_options_field_line(
    label: &str,
    value: &str,
    width: usize,
    value_color: Color,
) -> AnyElement<'static> {
    let value_width = width.saturating_sub(16);
    element! {
        View(width: width as u32, flex_direction: FlexDirection::Row) {
            View(width: 14) { Text(content: label, color: Color::Grey, wrap: TextWrap::NoWrap) }
            Text(content: truncate_account_options_line(value, value_width), color: value_color, wrap: TextWrap::NoWrap)
        }
    }
    .into_any()
}

fn credit_entitlement_label(usage: &CreditUsageStatus) -> String {
    match usage.provider_observation.availability() {
        CreditAvailability::Unknown => "Unknown".to_owned(),
        CreditAvailability::Depleted => "Depleted".to_owned(),
        CreditAvailability::Unlimited => "Unlimited".to_owned(),
        CreditAvailability::Available {
            balance: Some(balance),
        } => format!("Available · {}", balance.as_str()),
        CreditAvailability::Available { balance: None } => {
            "Available · balance withheld".to_owned()
        }
    }
}

fn credit_freshness_label(freshness: CreditUsageFreshness) -> &'static str {
    match freshness {
        CreditUsageFreshness::Fresh => "Fresh",
        CreditUsageFreshness::Stale => "Stale",
        CreditUsageFreshness::Unknown => "Unknown",
    }
}

fn freshness_color(freshness: CreditUsageFreshness) -> Color {
    match freshness {
        CreditUsageFreshness::Fresh => Color::Green,
        CreditUsageFreshness::Stale => Color::Yellow,
        CreditUsageFreshness::Unknown => Color::Grey,
    }
}

fn provider_control_note(usage: &CreditUsageStatus) -> Option<String> {
    use codex_router_core::credit_usage::CreditSpendControl;

    match usage.provider_observation.spend_control() {
        CreditSpendControl::Reached => Some("Provider spend control is reached.".to_owned()),
        CreditSpendControl::Unknown => Some("Provider spend-control state is unknown.".to_owned()),
        CreditSpendControl::Clear | CreditSpendControl::Unreported => usage
            .provider_observation
            .limit_reason()
            .filter(|reason| reason.blocks_credit_usage())
            .map(|reason| format!("Provider reports {}.", reason.as_str().replace('_', " "))),
    }
}

fn truncate_account_options_line(value: &str, width: usize) -> String {
    let line = value.replace('\n', " ");
    if line.chars().count() <= width {
        return line;
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    format!("{}…", line.chars().take(width - 1).collect::<String>())
}
