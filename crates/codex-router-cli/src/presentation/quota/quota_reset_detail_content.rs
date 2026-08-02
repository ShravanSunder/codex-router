use chrono::Local;
use chrono::TimeZone;
use iocraft::prelude::*;

use crate::quota_reset::reset_session_supervisor::OperationActivity;
use crate::quota_reset::reset_session_supervisor::OperationSuccess;
use crate::quota_reset::reset_session_supervisor::ResetCreditDisplayStatusDto;
use crate::quota_reset::reset_session_supervisor::ResetEligibilityDisabledReason;

use super::quota_reset_detail_rendering::ResetDetailRow;
use super::quota_reset_detail_rendering::StyledText;

pub(super) fn activity_field(
    label: &str,
    activity: &OperationActivity<OperationSuccess>,
    spinner: &str,
) -> ResetDetailRow {
    ResetDetailRow::Field {
        label: label.to_owned(),
        value: activity_label(activity, spinner),
        value_color: Color::White,
    }
}

fn activity_label(activity: &OperationActivity<OperationSuccess>, spinner: &str) -> String {
    match activity {
        OperationActivity::NotStarted => "waiting".to_owned(),
        OperationActivity::Loading => format!("{spinner} checking"),
        OperationActivity::Refreshing { previous } => format!(
            "{spinner} refreshing{}",
            previous
                .as_ref()
                .map_or("", |_| " · previous result visible")
        ),
        OperationActivity::Succeeded(_) => "ready".to_owned(),
        OperationActivity::Failed { failure, previous } => format!(
            "failed: {}{}",
            failure.message(),
            previous
                .as_ref()
                .map_or("", |_| " · previous result retained")
        ),
        OperationActivity::Cancelled => "cancelled".to_owned(),
        OperationActivity::RequestDispatchedAwaitingOutcome => {
            format!("{spinner} awaiting definitive outcome")
        }
    }
}

pub(super) fn format_credit_expiry(expires_unix_seconds: Option<i64>) -> String {
    match expires_unix_seconds {
        Some(value) => format!("expires {}", format_unix_local(value)),
        None => "does not expire".to_owned(),
    }
}

pub(super) fn format_credit_expiry_value(expires_unix_seconds: Option<i64>) -> String {
    expires_unix_seconds.map_or_else(|| "Does not expire".to_owned(), format_unix_local)
}

pub(super) fn format_unix_local(unix_seconds: i64) -> String {
    format_unix_in_timezone(unix_seconds, &Local)
}

fn format_unix_in_timezone<TTimeZone>(unix_seconds: i64, timezone: &TTimeZone) -> String
where
    TTimeZone: TimeZone,
    TTimeZone::Offset: std::fmt::Display,
{
    timezone
        .timestamp_opt(unix_seconds, 0)
        .single()
        .map_or_else(
            || "invalid expiry".to_owned(),
            |date_time| date_time.format("%Y-%m-%d %H:%M %:z").to_string(),
        )
}

pub(super) fn disabled_reason(reason: &ResetEligibilityDisabledReason) -> String {
    match reason {
        ResetEligibilityDisabledReason::LiveInspectionIncomplete => {
            "Live inspection is incomplete.".to_owned()
        }
        ResetEligibilityDisabledReason::WeeklyRemainingNotBelowTenPercentOrCreditNotExpiringSoon {
            remaining_percent,
        } => format!(
            "{remaining_percent}% remains; below 10% or credit expiry within 12h is required."
        ),
        ResetEligibilityDisabledReason::NoUsableCredit => {
            "No usable live reset credit is available.".to_owned()
        }
        ResetEligibilityDisabledReason::AuthorityUnavailable => {
            "Reset authority is unavailable.".to_owned()
        }
        ResetEligibilityDisabledReason::PinnedTargetInvalidated(reason) => {
            format!("The selected account changed ({reason:?}).")
        }
    }
}

pub(super) fn credit_status_label(status: ResetCreditDisplayStatusDto) -> &'static str {
    match status {
        ResetCreditDisplayStatusDto::Available => "available",
        ResetCreditDisplayStatusDto::Redeeming => "redeeming",
        ResetCreditDisplayStatusDto::Redeemed => "redeemed",
    }
}

pub(super) fn saved_credit_count(value: &str) -> Option<usize> {
    value
        .split_whitespace()
        .find_map(|part| part.parse::<usize>().ok())
}

pub(super) fn heading(content: impl Into<String>) -> StyledText {
    StyledText {
        content: content.into(),
        color: Color::Cyan,
        weight: Weight::Bold,
    }
}

pub(super) fn normal(content: impl Into<String>) -> ResetDetailRow {
    styled_row(content, Color::White)
}

pub(super) fn muted_text(content: impl Into<String>) -> ResetDetailRow {
    styled_row(content, Color::Grey)
}

pub(super) fn warning_text(content: impl Into<String>) -> ResetDetailRow {
    styled_row(content, Color::Yellow)
}

pub(super) fn accent_text(content: impl Into<String>) -> ResetDetailRow {
    styled_row(content, Color::Cyan)
}

fn styled_row(content: impl Into<String>, color: Color) -> ResetDetailRow {
    StyledText {
        content: content.into(),
        color,
        weight: Weight::Normal,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::format_unix_in_timezone;

    #[test]
    fn reset_credit_expiry_formats_the_same_instant_in_the_requested_local_offset() {
        // Arrange
        let eastern_daylight_time = FixedOffset::west_opt(4 * 60 * 60)
            .unwrap_or_else(|| panic!("four-hour west offset should be valid"));

        // Act
        let formatted = format_unix_in_timezone(1_900_000_000, &eastern_daylight_time);

        // Assert
        assert_eq!(formatted, "2030-03-17 13:46 -04:00");
    }
}
