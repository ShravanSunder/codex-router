//! Claude Messages evidence classification.

use codex_router_core::attempt_outcome::AttemptOutcome;
use codex_router_core::attempt_outcome::PassThroughReason;
use codex_router_core::route_profile::WindowKind;
use serde_json::Value;

use super::forward::ErrorBodyEvidence;
use crate::headers::HeaderCollection;

/// Classifies provider evidence before the Claude response is committed.
#[must_use]
pub(crate) fn classify(
    status: u16,
    headers: &HeaderCollection,
    evidence: ErrorBodyEvidence<'_>,
) -> AttemptOutcome {
    if (200..300).contains(&status) {
        return AttemptOutcome::Success;
    }

    if status == 429 {
        if let Some(window) = shared_rejected_window(headers) {
            return AttemptOutcome::SharedWindowExhausted {
                windows: vec![window],
                resets: vec![reset_for_window(headers, window)],
            };
        }

        if has_model_or_overage_rejection(headers) {
            return AttemptOutcome::PassThrough(PassThroughReason::ModelOrOverageLimit);
        }

        if has_explicit_quota_attribution(headers) {
            return AttemptOutcome::PassThrough(PassThroughReason::MalformedEvidence);
        }

        return AttemptOutcome::PassThrough(PassThroughReason::UnattributedLimit);
    }

    if status == 401 {
        if complete_error_is_expired_oauth_token(evidence) {
            return AttemptOutcome::CredentialRejected;
        }

        return AttemptOutcome::PassThrough(PassThroughReason::RequestRejected);
    }

    match status {
        529 => AttemptOutcome::PassThrough(PassThroughReason::Overloaded),
        500..=599 => AttemptOutcome::PassThrough(PassThroughReason::ServerError),
        400..=499 => AttemptOutcome::PassThrough(PassThroughReason::RequestRejected),
        _ => AttemptOutcome::PassThrough(PassThroughReason::MalformedEvidence),
    }
}

fn shared_rejected_window(headers: &HeaderCollection) -> Option<WindowKind> {
    if header_value(headers, "anthropic-ratelimit-unified-status").as_deref() != Some("rejected") {
        return None;
    }

    let representative_claim =
        header_value(headers, "anthropic-ratelimit-unified-representative-claim")?;
    let (window, window_status_header) = match representative_claim.as_str() {
        "five_hour" => (
            WindowKind::FiveHour,
            "anthropic-ratelimit-unified-5h-status",
        ),
        "seven_day" => (WindowKind::Weekly, "anthropic-ratelimit-unified-7d-status"),
        _ => return None,
    };

    if header_value(headers, window_status_header)
        .is_some_and(|window_status| window_status != "rejected")
    {
        return None;
    }

    Some(window)
}

fn reset_for_window(headers: &HeaderCollection, window: WindowKind) -> Option<u64> {
    let reset_header = match window {
        WindowKind::FiveHour => "anthropic-ratelimit-unified-5h-reset",
        WindowKind::Weekly => "anthropic-ratelimit-unified-7d-reset",
    };

    header_value(headers, reset_header)?.parse().ok()
}

fn has_model_or_overage_rejection(headers: &HeaderCollection) -> bool {
    if header_value(headers, "anthropic-ratelimit-unified-7d_oi-status").as_deref()
        == Some("rejected")
        || header_value(headers, "anthropic-ratelimit-unified-overage-status").as_deref()
            == Some("rejected")
        || header_value(
            headers,
            "anthropic-ratelimit-unified-overage-disabled-reason",
        )
        .is_some_and(|reason| !reason.is_empty())
    {
        return true;
    }

    header_value(headers, "anthropic-ratelimit-unified-representative-claim").is_some_and(
        |representative_claim| {
            representative_claim.contains("overage")
                || matches!(
                    representative_claim.as_str(),
                    "seven_day_opus" | "seven_day_sonnet"
                )
        },
    )
}

fn has_explicit_quota_attribution(headers: &HeaderCollection) -> bool {
    [
        "anthropic-ratelimit-unified-status",
        "anthropic-ratelimit-unified-representative-claim",
        "anthropic-ratelimit-unified-5h-status",
        "anthropic-ratelimit-unified-7d-status",
    ]
    .into_iter()
    .any(|name| header_value(headers, name).is_some())
}

fn complete_error_is_expired_oauth_token(evidence: ErrorBodyEvidence<'_>) -> bool {
    if !evidence.complete {
        return false;
    }

    let Ok(body) = serde_json::from_slice::<Value>(evidence.prefix) else {
        return false;
    };
    let Some(error) = body.get("error") else {
        return false;
    };
    let is_authentication_error = error
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|error_type| error_type == "authentication_error");
    let identifies_oauth_token = error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(message_identifies_expired_or_invalid_oauth_token);

    is_authentication_error && identifies_oauth_token
}

fn message_identifies_expired_or_invalid_oauth_token(message: &str) -> bool {
    let normalized_message = message.to_ascii_lowercase();
    normalized_message.contains("oauth")
        && normalized_message.contains("token")
        && (normalized_message.contains("invalid") || normalized_message.contains("expired"))
}

fn header_value(headers: &HeaderCollection, name: &str) -> Option<String> {
    headers
        .value(name)
        .map(|value| value.trim().to_ascii_lowercase())
}

#[cfg(test)]
mod tests;
