//! Fail-closed conversion of provider payloads into reset domain facts.

use serde::Deserialize;

use super::LiveResetCredit;
use super::QuotaResetError;
use super::http::provider_response_error;

pub(super) fn reset_credits_from_response_body(
    body: &[u8],
) -> Result<Vec<LiveResetCredit>, QuotaResetError> {
    let details =
        serde_json::from_slice::<ResetCreditsResponse>(body).map_err(provider_response_error)?;
    reset_credits_from_response(details)
}

pub(super) fn reset_credits_from_response(
    details: ResetCreditsResponse,
) -> Result<Vec<LiveResetCredit>, QuotaResetError> {
    details
        .credits
        .into_iter()
        .map(|credit| {
            validate_credit_status(&credit.status)?;
            if credit.id.trim().is_empty() || credit.id.chars().any(char::is_control) {
                return Err(QuotaResetError::Response {
                    message: "reset credit id is invalid".to_owned(),
                });
            }
            if credit
                .title
                .as_deref()
                .is_some_and(|title| title.chars().any(char::is_control))
            {
                return Err(QuotaResetError::Response {
                    message: "reset credit title contains control characters".to_owned(),
                });
            }
            let expires_unix_seconds = credit
                .expires_at
                .as_deref()
                .map(parse_utc_rfc3339_unix_seconds)
                .transpose()?;
            Ok(LiveResetCredit {
                id: credit.id,
                status: credit.status,
                expires_unix_seconds,
                expires_at: credit.expires_at,
                title: credit.title,
            })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub(super) struct ResetCreditsResponse {
    credits: Vec<ResetCreditPayload>,
}

#[derive(Debug, Deserialize)]
struct ResetCreditPayload {
    id: String,
    status: String,
    expires_at: Option<String>,
    title: Option<String>,
}

pub(super) fn remaining_percent_from_used(used_percent: i64) -> Result<u32, QuotaResetError> {
    if !(0..=100).contains(&used_percent) {
        return Err(QuotaResetError::Response {
            message: "weekly used_percent must be between 0 and 100".to_owned(),
        });
    }
    u32::try_from(100 - used_percent).map_err(provider_response_error)
}

pub(super) fn validate_credit_status(status: &str) -> Result<(), QuotaResetError> {
    if matches!(status, "available" | "redeeming" | "redeemed") {
        return Ok(());
    }
    Err(QuotaResetError::Response {
        message: "unknown reset credit status".to_owned(),
    })
}

pub(super) fn parse_utc_rfc3339_unix_seconds(value: &str) -> Result<i64, QuotaResetError> {
    let value = value
        .strip_suffix('Z')
        .ok_or_else(|| QuotaResetError::Response {
            message: "reset-credit expiration must be a UTC RFC 3339 timestamp".to_owned(),
        })?;
    let (date, time) = value
        .split_once('T')
        .ok_or_else(|| QuotaResetError::Response {
            message: "reset-credit expiration must contain T".to_owned(),
        })?;
    let mut date_parts = date.split('-');
    let year = parse_timestamp_part(date_parts.next(), "year", 4)?;
    let month = parse_timestamp_part(date_parts.next(), "month", 2)?;
    let day = parse_timestamp_part(date_parts.next(), "day", 2)?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) {
        return Err(invalid_timestamp("date"));
    }
    let month_days = [
        31,
        28 + i64::from(is_leap_year(year)),
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let month_index = usize::try_from(month - 1).map_err(provider_response_error)?;
    if day < 1 || day > month_days.get(month_index).copied().unwrap_or(0) {
        return Err(invalid_timestamp("day"));
    }

    let mut time_parts = time.split(':');
    let hour = parse_timestamp_part(time_parts.next(), "hour", 2)?;
    let minute = parse_timestamp_part(time_parts.next(), "minute", 2)?;
    let second_part = time_parts
        .next()
        .ok_or_else(|| invalid_timestamp("second"))?;
    let mut second_parts = second_part.split('.');
    let second = parse_timestamp_part(second_parts.next(), "second", 2)?;
    if let Some(fraction) = second_parts.next()
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(invalid_timestamp("fraction"));
    }
    if second_parts.next().is_some()
        || time_parts.next().is_some()
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return Err(invalid_timestamp("time"));
    }

    let days = days_since_unix_epoch(year, month, day);
    days.checked_mul(86_400)
        .and_then(|seconds| seconds.checked_add(hour * 3_600 + minute * 60 + second))
        .ok_or_else(|| invalid_timestamp("range"))
}

fn parse_timestamp_part(
    value: Option<&str>,
    name: &str,
    expected_width: usize,
) -> Result<i64, QuotaResetError> {
    let value = value.ok_or_else(|| invalid_timestamp(name))?;
    if value.len() != expected_width || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_timestamp(name));
    }
    value.parse::<i64>().map_err(provider_response_error)
}

const fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const fn days_since_unix_epoch(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = year - if month <= 2 { 1 } else { 0 };
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn invalid_timestamp(part: &str) -> QuotaResetError {
    QuotaResetError::Response {
        message: format!("invalid reset-credit expiration {part}"),
    }
}
