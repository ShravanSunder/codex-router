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
        Some(value) => format!("expires {}", format_unix_utc(value)),
        None => "does not expire".to_owned(),
    }
}

pub(super) fn format_credit_expiry_value(expires_unix_seconds: Option<i64>) -> String {
    expires_unix_seconds.map_or_else(|| "Does not expire".to_owned(), format_unix_utc)
}

fn format_unix_utc(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds_in_day = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_date_from_unix_days(days);
    let hour = seconds_in_day / 3_600;
    let minute = seconds_in_day % 3_600 / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

fn civil_date_from_unix_days(unix_days: i64) -> (i64, i64, i64) {
    let shifted_days = unix_days + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
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
