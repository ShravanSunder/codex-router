use iocraft::prelude::*;

use crate::quota_reset::reset_session_supervisor::ConfirmationSelection;
use crate::quota_reset::reset_session_supervisor::KnownConsumeOutcome;
use crate::quota_reset::reset_session_supervisor::OperationActivity;
use crate::quota_reset::reset_session_supervisor::OperationSuccess;
use crate::quota_reset::reset_session_supervisor::ResetCreditDisplayStatusDto;
use crate::quota_reset::reset_session_supervisor::ResetEligibilityDisabledReason;
use crate::quota_reset::reset_session_supervisor::ResetValueProvenance;
use crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot;
use crate::quota_reset::reset_session_supervisor::WorkflowPhase;
use crate::quota_reset::reset_session_supervisor::WorkflowResult;

use super::quota_browse_rendering::fit_line;
use super::quota_reset_presentation_model::ResetPaneTarget;
use super::responsive_quota_layout::quota_spinner_tick;

pub(super) fn reset_panel_content_height(snapshot: &ResetWorkflowSnapshot) -> usize {
    if snapshot.phase() == WorkflowPhase::Result {
        7
    } else {
        21
    }
}

pub(super) fn render_reset_panel(
    snapshot: &ResetWorkflowSnapshot,
    target: &ResetPaneTarget,
    width: usize,
    height: usize,
    inventory_page_start: usize,
    inventory_page_size: usize,
    spinner_tick: usize,
) -> AnyElement<'static> {
    let inner_width = width.saturating_sub(4).max(12);
    let spinner = quota_spinner_tick(spinner_tick);
    let mut lines = if snapshot.phase() == WorkflowPhase::Result {
        result_lines(snapshot.result())
    } else {
        reset_workflow_lines(
            snapshot,
            target,
            inventory_page_start,
            inventory_page_size,
            spinner,
        )
    };
    let children = lines
        .drain(..)
        .map(|(content, color, weight)| {
            element! {
                Text(content: fit_line(&content, inner_width), color, weight, wrap: TextWrap::NoWrap)
            }
            .into_any()
        })
        .collect::<Vec<_>>();
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
            #(children)
        }
    }
    .into_any()
}

fn reset_workflow_lines(
    snapshot: &ResetWorkflowSnapshot,
    target: &ResetPaneTarget,
    inventory_page_start: usize,
    inventory_page_size: usize,
    spinner: &str,
) -> Vec<(String, Color, Weight)> {
    let mut lines = vec![
        reset_heading("Reset credit"),
        reset_line(
            format!("{}  [{}]", target.account_label, target.account_tag),
            Color::White,
        ),
        reset_line(
            format!("saved weekly  {}", target.saved_weekly_window),
            Color::Grey,
        ),
        reset_line(
            format!("saved credits {}", target.saved_reset_credits),
            Color::Grey,
        ),
    ];
    lines.extend(operation_lines(snapshot, spinner));
    if let Some(live_weekly) = snapshot.live_weekly() {
        lines.push(reset_line(
            format!(
                "{} weekly  {}% remaining",
                provenance_label(live_weekly.provenance),
                live_weekly.remaining_percent
            ),
            Color::White,
        ));
    }
    let inventory = snapshot.credit_inventory();
    if !inventory.is_empty() {
        let page_end = inventory_page_start
            .saturating_add(inventory_page_size)
            .min(inventory.len());
        lines.push(reset_heading(format!(
            "Credits {} · {}-{} of {}",
            snapshot
                .credit_inventory_provenance()
                .map_or("live", provenance_label),
            inventory_page_start.saturating_add(1).min(inventory.len()),
            page_end,
            inventory.len()
        )));
        for credit in inventory
            .iter()
            .skip(inventory_page_start)
            .take(inventory_page_size)
        {
            let marker = if credit.earliest_usable {
                "earliest"
            } else {
                "credit"
            };
            let title = credit.title.as_deref().unwrap_or("untitled");
            let expiry = credit.expires_unix_seconds.map_or_else(
                || "non-expiring".to_owned(),
                |value| format!("expires {value}"),
            );
            lines.push(reset_line(
                format!(
                    "{marker} [{}] {title} · {} · {expiry}",
                    credit.id_hint,
                    credit_status_label(credit.status)
                ),
                if credit.earliest_usable {
                    Color::Yellow
                } else {
                    Color::White
                },
            ));
        }
        let remaining = inventory.len().saturating_sub(page_end);
        if remaining > 0 {
            lines.push(reset_line(
                format!("+{remaining} credits below"),
                Color::Grey,
            ));
        }
        if saved_credit_count(&target.saved_reset_credits)
            .is_some_and(|saved_count| saved_count != inventory.len())
        {
            lines.push(reset_line(
                format!(
                    "Warning: saved count and current live count disagree (saved {}, live {}).",
                    target.saved_reset_credits,
                    inventory.len()
                ),
                Color::Yellow,
            ));
        }
    }
    lines.extend(phase_lines(snapshot, target, spinner));
    lines
}

pub(super) fn reset_footer(snapshot: Option<&ResetWorkflowSnapshot>) -> &'static str {
    match snapshot.map(ResetWorkflowSnapshot::phase) {
        None | Some(WorkflowPhase::Browse) => {
            "↑/↓ focus  ctrl-r inspect reset credits  esc/q exit  ctrl-c exit"
        }
        Some(WorkflowPhase::Inspecting) => "esc/ctrl-r back  ctrl-c exit without consume",
        Some(WorkflowPhase::Inspected) => {
            "enter confirmation  pgup/pgdn credits  esc/ctrl-r back  ctrl-c exit without consume"
        }
        Some(WorkflowPhase::Confirming) => {
            "←/→ select  enter confirm  pgup/pgdn credits  esc/ctrl-r cancel  ctrl-c exit without consume"
        }
        Some(WorkflowPhase::Revalidating) => "esc/ctrl-r cancel before consume",
        Some(WorkflowPhase::Committing) => "consuming reset credit  waiting for definitive result",
        Some(WorkflowPhase::Result) => "enter/esc/ctrl-r back to quota browse",
    }
}

fn operation_lines(
    snapshot: &ResetWorkflowSnapshot,
    spinner: &str,
) -> Vec<(String, Color, Weight)> {
    let activities = snapshot.activities();
    [
        ("inspect usage", &activities.inspection_live_usage),
        ("inspect credits", &activities.inspection_credit_inventory),
        ("revalidate usage", &activities.revalidation_live_usage),
        (
            "revalidate credits",
            &activities.revalidation_credit_inventory,
        ),
        ("consume credit", &activities.consume_credit),
    ]
    .into_iter()
    .map(|(label, activity)| {
        reset_line(
            format!("{label:<18} {}", activity_label(activity, spinner)),
            Color::White,
        )
    })
    .collect()
}

fn activity_label(activity: &OperationActivity<OperationSuccess>, spinner: &str) -> String {
    match activity {
        OperationActivity::NotStarted => "not started".to_owned(),
        OperationActivity::Loading => format!("{spinner} loading"),
        OperationActivity::Refreshing { previous } => format!(
            "{spinner} refreshing{}",
            previous
                .as_ref()
                .map_or("", |_| " · previous result visible")
        ),
        OperationActivity::Succeeded(_) => "succeeded".to_owned(),
        OperationActivity::Failed { failure, previous } => format!(
            "failed: {}{}",
            failure.message(),
            previous
                .as_ref()
                .map_or("", |_| " · previous result retained")
        ),
        OperationActivity::Cancelled => "cancelled".to_owned(),
        OperationActivity::RequestDispatchedAwaitingOutcome => {
            format!("{spinner} request dispatched · awaiting definitive outcome")
        }
    }
}

fn phase_lines(
    snapshot: &ResetWorkflowSnapshot,
    target: &ResetPaneTarget,
    spinner: &str,
) -> Vec<(String, Color, Weight)> {
    match snapshot.phase() {
        WorkflowPhase::Browse | WorkflowPhase::Inspecting => Vec::new(),
        WorkflowPhase::Inspected => vec![reset_line(
            "Inspection complete. Enter opens confirmation.".to_owned(),
            Color::Cyan,
        )],
        WorkflowPhase::Confirming => confirmation_lines(snapshot, target),
        WorkflowPhase::Revalidating => vec![reset_line(
            format!(
                "{spinner} Revalidating account, credentials, live usage, and selected credit..."
            ),
            Color::Yellow,
        )],
        WorkflowPhase::Committing => vec![reset_line(
            format!("{spinner} Consuming reset credit... waiting for a definitive result."),
            Color::Yellow,
        )],
        WorkflowPhase::Result => Vec::new(),
    }
}

fn confirmation_lines(
    snapshot: &ResetWorkflowSnapshot,
    target: &ResetPaneTarget,
) -> Vec<(String, Color, Weight)> {
    let mut lines = vec![
        reset_heading("Confirm scarce reset credit"),
        reset_line(
            "Warning: confirming consumes one scarce reset credit.".to_owned(),
            Color::Yellow,
        ),
        reset_line(
            format!("Account {} [{}]", target.account_label, target.account_tag),
            Color::White,
        ),
    ];
    if let Some(weekly) = snapshot.live_weekly() {
        lines.push(reset_line(
            format!("Live weekly {}% remaining", weekly.remaining_percent),
            Color::White,
        ));
    }
    if let Some(credit) = snapshot.selected_credit() {
        lines.push(reset_line(
            format!(
                "Credit [{}] {} · {}",
                credit.id_hint,
                credit.title.as_deref().unwrap_or("untitled"),
                credit.expires_unix_seconds.map_or_else(
                    || "non-expiring".to_owned(),
                    |value| format!("expires {value}")
                )
            ),
            Color::White,
        ));
    }
    let no_marker = if snapshot.confirmation_selection() == ConfirmationSelection::No {
        "[No]"
    } else {
        " No "
    };
    let yes_marker = if snapshot.yes_enabled()
        && snapshot.confirmation_selection() == ConfirmationSelection::Yes
    {
        "[Yes]"
    } else if snapshot.yes_enabled() {
        " Yes "
    } else {
        " Yes disabled "
    };
    lines.push(reset_line(
        format!("{no_marker}   {yes_marker}"),
        Color::Yellow,
    ));
    if let Some(reason) = snapshot.disabled_yes_reason() {
        lines.push(reset_line(disabled_reason(reason), Color::Grey));
    }
    lines
}

fn result_lines(result: Option<&WorkflowResult>) -> Vec<(String, Color, Weight)> {
    let (heading, summary, assurance, color) = match result {
        Some(WorkflowResult::Known(KnownConsumeOutcome::Reset { windows_reset })) => (
            "SUCCESS — RESET COMPLETED",
            format!("Provider confirmed: {windows_reset} quota windows reset."),
            "One reset credit was consumed.",
            Color::Green,
        ),
        Some(WorkflowResult::Known(KnownConsumeOutcome::NothingToReset)) => (
            "DEFINITIVE PROVIDER RESULT",
            "Provider reports nothing to reset.".to_owned(),
            "The provider returned a definitive response.",
            Color::Yellow,
        ),
        Some(WorkflowResult::Known(KnownConsumeOutcome::NoCredit)) => (
            "DEFINITIVE PROVIDER RESULT",
            "Provider reports no reset credit.".to_owned(),
            "The provider returned a definitive response.",
            Color::Yellow,
        ),
        Some(WorkflowResult::Known(KnownConsumeOutcome::AlreadyRedeemed)) => (
            "DEFINITIVE PROVIDER RESULT",
            "Provider reports credit already redeemed.".to_owned(),
            "The provider returned a definitive response.",
            Color::Yellow,
        ),
        Some(WorkflowResult::OutcomeUnknown(reason)) => (
            "OUTCOME UNKNOWN — DO NOT RETRY",
            format!("No definitive response: {}.", reason.message()),
            "The credit may have been consumed. Refresh live credits before deciding what to do next.",
            Color::Red,
        ),
        Some(WorkflowResult::Refused(reason)) => (
            "NOT CONSUMED",
            format!("Reset refused before consume: {}.", reason.message()),
            "No consume request was sent. No reset credit was consumed.",
            Color::Yellow,
        ),
        None => (
            "RESULT UNAVAILABLE",
            "Reset result unavailable.".to_owned(),
            "Do not retry until live credits have been inspected again.",
            Color::Red,
        ),
    };
    vec![
        (heading.to_owned(), color, Weight::Bold),
        reset_line(summary, Color::White),
        reset_line(assurance.to_owned(), color),
        reset_line(
            "Saved quota may remain stale until the normal quota refresh updates it.".to_owned(),
            Color::Grey,
        ),
        reset_line(
            "Enter, Esc, or Ctrl-R returns to quota status.".to_owned(),
            Color::Cyan,
        ),
    ]
}

fn disabled_reason(reason: &ResetEligibilityDisabledReason) -> String {
    match reason {
        ResetEligibilityDisabledReason::LiveInspectionIncomplete => {
            "Yes disabled: current live inspection is incomplete.".to_owned()
        }
        ResetEligibilityDisabledReason::WeeklyRemainingNotBelowOnePercent { remaining_percent } => {
            format!(
                "Yes disabled: live weekly remaining is {remaining_percent}%; less than 1% is required."
            )
        }
        ResetEligibilityDisabledReason::NoUsableCredit => {
            "Yes disabled: no usable live reset credit.".to_owned()
        }
        ResetEligibilityDisabledReason::AuthorityUnavailable => {
            "Yes disabled: reset authority is unavailable.".to_owned()
        }
        ResetEligibilityDisabledReason::PinnedTargetInvalidated(reason) => {
            format!("Yes disabled: pinned target invalidated ({reason:?}).")
        }
    }
}

fn provenance_label(provenance: ResetValueProvenance) -> &'static str {
    match provenance {
        ResetValueProvenance::CurrentLive => "current live",
        ResetValueProvenance::PreviousLiveRefreshing => "previous live · refreshing",
    }
}

fn credit_status_label(status: ResetCreditDisplayStatusDto) -> &'static str {
    match status {
        ResetCreditDisplayStatusDto::Available => "available",
        ResetCreditDisplayStatusDto::Redeeming => "redeeming",
        ResetCreditDisplayStatusDto::Redeemed => "redeemed",
    }
}

fn saved_credit_count(value: &str) -> Option<usize> {
    value
        .split_whitespace()
        .find_map(|part| part.parse::<usize>().ok())
}

fn reset_heading(content: impl Into<String>) -> (String, Color, Weight) {
    (content.into(), Color::Cyan, Weight::Bold)
}

fn reset_line(content: String, color: Color) -> (String, Color, Weight) {
    (content, color, Weight::Normal)
}
