use super::super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::quota) enum QuotaHumanGroup {
    Preferred,
    Available,
    Held,
    BlockedOrStale,
}

pub(in crate::quota) fn quota_human_group(row: &QuotaStatusRow) -> QuotaHumanGroup {
    if row.preferred_next {
        return QuotaHumanGroup::Preferred;
    }
    if row.freshness != QuotaEvidenceFreshness::Fresh {
        return QuotaHumanGroup::BlockedOrStale;
    }
    match row.routing_reason {
        RoutingReason::AvailableSamePool => QuotaHumanGroup::Available,
        RoutingReason::HeldReserve
        | RoutingReason::HeldUnknown
        | RoutingReason::HeldShortWindowGuard => QuotaHumanGroup::Held,
        RoutingReason::UnknownFallbackAvailable => QuotaHumanGroup::BlockedOrStale,
        _ => match row.availability {
            AccountAvailability::Usable => QuotaHumanGroup::Available,
            AccountAvailability::Reserve => QuotaHumanGroup::Held,
            AccountAvailability::Retiring
            | AccountAvailability::Blocked
            | AccountAvailability::Unknown
            | AccountAvailability::Excluded => QuotaHumanGroup::BlockedOrStale,
        },
    }
}

pub(in crate::quota) fn quota_state_text(row: &QuotaStatusRow) -> &'static str {
    if row.preferred_next {
        return "preferred";
    }
    if row.freshness == QuotaEvidenceFreshness::Stale {
        return "stale";
    }
    if row.freshness == QuotaEvidenceFreshness::Unknown {
        return "unknown";
    }
    match quota_human_group(row) {
        QuotaHumanGroup::Preferred => "preferred",
        QuotaHumanGroup::Available => "available",
        QuotaHumanGroup::Held => "held",
        QuotaHumanGroup::BlockedOrStale => "blocked",
    }
}

pub(in crate::quota) fn quota_window_visual_summary(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    label: &'static str,
    now_unix_seconds: u64,
) -> String {
    let Some(window) = windows
        .iter()
        .find(|window| window.window_seconds == window_seconds)
    else {
        return format!("{label} {} no data", quota_bar(0, true));
    };
    let note = window_display_note(window, now_unix_seconds)
        .replace("resets in ", "reset ")
        .replace("resets ", "reset ");
    format!(
        "{label} {} {} left, {note}",
        quota_bar(window.remaining_headroom, true),
        format_percent(window.remaining_headroom)
    )
}

pub(in crate::quota) fn quota_account_list_window_summary(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    label: &'static str,
    now_unix_seconds: u64,
) -> String {
    let Some(window) = windows
        .iter()
        .find(|window| window.window_seconds == window_seconds)
    else {
        return format!("{} no data {label}", quota_bar(0, true));
    };
    let reset_note = window_display_note(window, now_unix_seconds).replace("resets in ", "resets ");
    format!(
        "{} {} {label} · {}",
        quota_bar(window.remaining_headroom, true),
        format_percent(window.remaining_headroom),
        reset_note
    )
}

pub(in crate::quota) fn first_line(value: &str) -> &str {
    value.lines().next().unwrap_or(value).trim()
}

pub(in crate::quota) fn active_clients_label(row: &QuotaStatusRow) -> String {
    row.active_clients_value.map_or_else(
        || "unknown clients".to_owned(),
        |count| {
            if count == 1 {
                "1 client".to_owned()
            } else {
                format!("{count} clients")
            }
        },
    )
}

pub(in crate::quota) fn reset_credits_account_list_label(
    reset_credits_available: Option<u32>,
) -> String {
    reset_credits_available.map_or_else(
        || "resets unknown".to_owned(),
        |credits| {
            if credits == 1 {
                "1 reset".to_owned()
            } else {
                format!("{credits} resets")
            }
        },
    )
}

pub(in crate::quota) fn reason_summary(row: &QuotaStatusRow) -> String {
    first_line(&row.routing)
        .replace("preferred by quota: ", "")
        .replace("available by quota: ", "")
        .replace("held by quota: ", "")
        .replace("fallback by quota: ", "")
        .replace("blocked: ", "")
}

pub(in crate::quota) fn format_window_cell(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    now_unix_seconds: u64,
    unicode: bool,
) -> String {
    let Some(window) = windows
        .iter()
        .find(|window| window.window_seconds == window_seconds)
    else {
        return format!("{} no data\nneeds refresh", quota_bar(0, unicode));
    };
    format!(
        "{} {} left\n{}",
        quota_bar(window.remaining_headroom, unicode),
        format_percent(window.remaining_headroom),
        window_display_note(window, now_unix_seconds)
    )
}

pub(in crate::quota) fn quota_bar(percent: u32, unicode: bool) -> String {
    let filled = percent.min(100).div_ceil(10) as usize;
    let empty = 10_usize.saturating_sub(filled);
    if unicode {
        format!("{}{}", "█".repeat(filled), "░".repeat(empty))
    } else {
        format!("{}{}", "#".repeat(filled), "-".repeat(empty))
    }
}

pub(in crate::quota) fn format_reset_credits(reset_credits_available: Option<u32>) -> String {
    reset_credits_available.map_or_else(
        || "-".to_owned(),
        |credits| {
            if credits == 1 {
                "1 available".to_owned()
            } else {
                format!("{credits} available")
            }
        },
    )
}

pub(in crate::quota) fn format_active_clients(active_clients: ActiveClientMirrorStatus) -> String {
    match active_clients {
        ActiveClientMirrorStatus::MirrorFresh {
            count,
            pressure: _,
            max_age_seconds,
        } => {
            let count_text = if count == 1 {
                "1 client".to_owned()
            } else {
                format!("{count} clients")
            };
            format!(
                "{count_text}\nmirror <= {}",
                format_duration(max_age_seconds)
            )
        }
        ActiveClientMirrorStatus::Unavailable => "unknown\nmirror unavailable".to_owned(),
    }
}

pub(in crate::quota) fn format_refresh_status(
    refresh_status: Option<&QuotaRefreshStatusView>,
    now_unix_seconds: u64,
) -> String {
    let Some(refresh_status) = refresh_status else {
        return "never\nneeds refresh".to_owned();
    };
    match refresh_status.status_source() {
        QuotaRefreshStatusSource::LegacyMissingRefreshStatus => "legacy\nneeds refresh".to_owned(),
        QuotaRefreshStatusSource::Recorded => {
            let success = refresh_status.last_success_unix_seconds().map_or_else(
                || "no success".to_owned(),
                |last_success| {
                    format!(
                        "ok {}",
                        format_relative_time(last_success, now_unix_seconds)
                    )
                },
            );
            if let Some(error_class) = refresh_status.last_error_class() {
                let attempt = refresh_status.last_attempt_unix_seconds().map_or_else(
                    || "attempt unknown".to_owned(),
                    |last_attempt| {
                        format!(
                            "failed {}",
                            format_relative_time(last_attempt, now_unix_seconds)
                        )
                    },
                );
                format!(
                    "{success}\n{attempt}: {}",
                    quota_refresh_error_class_label(error_class)
                )
            } else {
                success
            }
        }
    }
}

pub(in crate::quota) const fn quota_refresh_error_class_label(
    error_class: QuotaRefreshErrorClass,
) -> &'static str {
    match error_class {
        QuotaRefreshErrorClass::AuthError => "auth",
        QuotaRefreshErrorClass::NetworkError => "network",
        QuotaRefreshErrorClass::ProviderError => "provider",
        QuotaRefreshErrorClass::ParseError => "parse",
        QuotaRefreshErrorClass::RateLimited => "rate limited",
    }
}

pub(in crate::quota) fn format_pace_cell(
    windows: &[DisplayQuotaWindow],
    assessment: &BurnDownAccountAssessment,
    now_unix_seconds: u64,
) -> String {
    if matches!(
        assessment.quota_evidence_reason(),
        QuotaEvidenceReason::NeedsQuotaProbe
            | QuotaEvidenceReason::MissingExpectedWindow
            | QuotaEvidenceReason::UnknownQuotaWindow
            | QuotaEvidenceReason::MissingResetTime
    ) {
        return "needs refresh".to_owned();
    }
    let short = format_window_pace(windows, V1_SHORT_WINDOW_SECONDS, "5h", now_unix_seconds);
    let weekly = format_window_pace(
        windows,
        V1_WEEKLY_WINDOW_SECONDS,
        "weekly",
        now_unix_seconds,
    );
    format!("{short}\n{weekly}")
}

pub(in crate::quota) fn format_burn_cell(assessment: &BurnDownAccountAssessment) -> String {
    if matches!(
        assessment.quota_evidence_reason(),
        QuotaEvidenceReason::NeedsQuotaProbe
            | QuotaEvidenceReason::MissingExpectedWindow
            | QuotaEvidenceReason::UnknownQuotaWindow
            | QuotaEvidenceReason::MissingResetTime
    ) {
        return "needs refresh".to_owned();
    }

    let quota_guard = format!(
        "quota guard 5h {}% / weekly {}%",
        assessment.short_pressure(),
        assessment.long_pressure()
    );
    if assessment.routing_weight().is_some() {
        quota_guard
    } else {
        format!("not selectable\n{quota_guard}")
    }
}

pub(in crate::quota) fn format_window_pace(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    label: &'static str,
    now_unix_seconds: u64,
) -> String {
    let Some(window) = windows
        .iter()
        .find(|window| window.window_seconds == window_seconds)
    else {
        return format!("{label} needs refresh");
    };
    match window.status {
        QuotaWindowStatus::Unknown => format!("{label} needs refresh"),
        QuotaWindowStatus::Ineligible if window.remaining_headroom == 0 => {
            format!("{label} ineligible")
        }
        QuotaWindowStatus::Ineligible => format!("{label} ineligible"),
        QuotaWindowStatus::Eligible | QuotaWindowStatus::Stale => {
            let (pressure, surplus) = window_pressure_and_surplus(window, now_unix_seconds);
            let pace = match (pressure.unwrap_or(0), surplus.unwrap_or(0)) {
                (0, 0) => format!("{label} on pace"),
                (behind, 0) => format!("{label} {behind}% behind"),
                (0, ahead) => format!("{label} {ahead}% ahead"),
                _ => format!("{label} needs refresh"),
            };
            format!(
                "{pace}; {}",
                format_run_rate_estimate(window.run_rate_estimate, now_unix_seconds)
            )
        }
    }
}

pub(in crate::quota) fn format_run_rate_estimate(
    estimate: QuotaRunRateEstimate,
    now_unix_seconds: u64,
) -> String {
    match estimate.confidence() {
        QuotaRunRateConfidence::Unknown => "history unknown".to_owned(),
        QuotaRunRateConfidence::Insufficient => "history insufficient".to_owned(),
        QuotaRunRateConfidence::Stale => "history stale".to_owned(),
        QuotaRunRateConfidence::Low | QuotaRunRateConfidence::Normal => {
            let confidence = run_rate_confidence_label(estimate.confidence());
            let burn_rate = format_burn_rate_basis_points_per_hour(
                estimate.burn_rate_basis_points_per_hour().unwrap_or(0),
            );
            match estimate.projected_exhaustion_unix_seconds(now_unix_seconds) {
                Some(runout) => {
                    format!(
                        "{confidence} burn {burn_rate}; runout {}",
                        format_relative_time(runout, now_unix_seconds)
                    )
                }
                None => format!("{confidence} burn {burn_rate}; no runout"),
            }
        }
    }
}

pub(in crate::quota) fn format_burn_rate_basis_points_per_hour(
    burn_rate_basis_points_per_hour: u32,
) -> String {
    let whole_percent = burn_rate_basis_points_per_hour / 100;
    let fractional_basis_points = burn_rate_basis_points_per_hour % 100;
    if fractional_basis_points == 0 {
        return format!("{whole_percent}%/h");
    }
    if fractional_basis_points.is_multiple_of(10) {
        return format!("{}.{}%/h", whole_percent, fractional_basis_points / 10);
    }

    format!("{whole_percent}.{fractional_basis_points:02}%/h")
}

pub(in crate::quota) const fn run_rate_confidence_label(
    confidence: QuotaRunRateConfidence,
) -> &'static str {
    match confidence {
        QuotaRunRateConfidence::Unknown => "unknown",
        QuotaRunRateConfidence::Insufficient => "insufficient",
        QuotaRunRateConfidence::Low => "low",
        QuotaRunRateConfidence::Normal => "normal",
        QuotaRunRateConfidence::Stale => "stale",
    }
}

pub(in crate::quota) fn format_routing_cell(assessment: &BurnDownAccountAssessment) -> String {
    let first_line = format_routing_reason(assessment.routing_reason());
    if let Some(limiting_window) = assessment.limiting_window() {
        format!(
            "{first_line}\nlimiting window: {} {} left",
            quota_window_label(limiting_window.window_seconds()),
            format_percent(limiting_window.remaining_headroom())
        )
    } else {
        first_line.to_owned()
    }
}

pub(in crate::quota) const fn routing_reason_is_preferred(reason: RoutingReason) -> bool {
    matches!(
        reason,
        RoutingReason::PreferredNearResetDrainable
            | RoutingReason::PreferredNearResetControlledDrain
            | RoutingReason::PreferredWeeklyHealthier
            | RoutingReason::PreferredWeeklyResetSoon
            | RoutingReason::PreferredShortResetSoon
            | RoutingReason::PreferredProjectedBurn
            | RoutingReason::PreferredSafestQuota
            | RoutingReason::PreferredLastResortShortWindowGuard
    )
}

pub(in crate::quota) fn format_routing_reason(reason: RoutingReason) -> &'static str {
    match reason {
        RoutingReason::PreferredNearResetDrainable => "preferred by quota: near-reset drainable",
        RoutingReason::PreferredNearResetControlledDrain => {
            "preferred by quota: near-reset controlled drain"
        }
        RoutingReason::PreferredWeeklyHealthier => "preferred by quota: weekly healthier",
        RoutingReason::PreferredWeeklyResetSoon => "preferred by quota: weekly reset soon",
        RoutingReason::PreferredShortResetSoon => "preferred by quota: 5h reset soon",
        RoutingReason::PreferredProjectedBurn => "preferred by quota: projected burn",
        RoutingReason::PreferredSafestQuota => "preferred by quota: safest quota",
        RoutingReason::PreferredLastResortShortWindowGuard => {
            "preferred by quota: last-resort 5h guard"
        }
        RoutingReason::AvailableSamePool => "available by quota: same pool",
        RoutingReason::HeldReserve => "held by quota: reserve",
        RoutingReason::HeldUnknown => "held by quota: needs refresh",
        RoutingReason::HeldShortWindowGuard => "held by quota: 5h guard",
        RoutingReason::UnknownFallbackPreferred => "fallback by quota: needs refresh",
        RoutingReason::UnknownFallbackAvailable => "fallback by quota: same unknown pool",
        RoutingReason::RetiringNearZero => "retiring: near zero quota",
        RoutingReason::ExcludedDisabled => "excluded: account disabled",
        RoutingReason::ExcludedMissingCredential => "excluded: missing credential",
        RoutingReason::ExcludedWeeklyQuotaFloor => "blocked: weekly quota floor",
        RoutingReason::BlockedWindowExhausted => "blocked: quota empty",
        RoutingReason::BlockedWindowIneligible => "blocked: quota ineligible",
    }
}

pub(in crate::quota) fn format_next_use(assessment: &BurnDownAccountAssessment) -> &'static str {
    format_next_use_from_routing_reason(assessment.routing_reason())
}

pub(in crate::quota) fn format_next_use_from_routing_reason(reason: RoutingReason) -> &'static str {
    match reason {
        RoutingReason::PreferredWeeklyHealthier
        | RoutingReason::PreferredNearResetDrainable
        | RoutingReason::PreferredNearResetControlledDrain
        | RoutingReason::PreferredWeeklyResetSoon
        | RoutingReason::PreferredShortResetSoon
        | RoutingReason::PreferredProjectedBurn
        | RoutingReason::PreferredSafestQuota
        | RoutingReason::PreferredLastResortShortWindowGuard => "preferred by quota",
        RoutingReason::AvailableSamePool => "available by quota",
        RoutingReason::HeldReserve
        | RoutingReason::HeldUnknown
        | RoutingReason::HeldShortWindowGuard => "held by quota",
        RoutingReason::UnknownFallbackPreferred | RoutingReason::UnknownFallbackAvailable => {
            "fallback by quota"
        }
        RoutingReason::RetiringNearZero => "retiring",
        RoutingReason::ExcludedDisabled
        | RoutingReason::ExcludedMissingCredential
        | RoutingReason::ExcludedWeeklyQuotaFloor
        | RoutingReason::BlockedWindowExhausted
        | RoutingReason::BlockedWindowIneligible => "blocked",
    }
}

pub(in crate::quota) fn format_percent(value: u32) -> String {
    format!("{}%", value.min(100))
}

pub(in crate::quota) fn format_relative_time(
    target_unix_seconds: u64,
    now_unix_seconds: u64,
) -> String {
    if target_unix_seconds >= now_unix_seconds {
        format!(
            "in {}",
            format_duration(target_unix_seconds.saturating_sub(now_unix_seconds))
        )
    } else {
        format!(
            "{} ago",
            format_duration(now_unix_seconds.saturating_sub(target_unix_seconds))
        )
    }
}

pub(in crate::quota) fn format_duration(seconds: u64) -> String {
    pub(super) const MINUTE: u64 = 60;
    pub(super) const HOUR: u64 = 60 * MINUTE;
    pub(super) const DAY: u64 = 24 * HOUR;

    if seconds >= DAY {
        let days = seconds / DAY;
        let hours = (seconds % DAY) / HOUR;
        if hours == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {hours}h")
        }
    } else if seconds >= HOUR {
        let hours = seconds / HOUR;
        let minutes = (seconds % HOUR) / MINUTE;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    } else if seconds >= MINUTE {
        let minutes = seconds / MINUTE;
        let remaining_seconds = seconds % MINUTE;
        if remaining_seconds == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {remaining_seconds}s")
        }
    } else {
        format!("{seconds}s")
    }
}

pub(in crate::quota) const fn quota_window_status_from_selector_status(
    status: SelectorQuotaWindowStatus,
) -> QuotaWindowStatus {
    match status {
        SelectorQuotaWindowStatus::Eligible => QuotaWindowStatus::Eligible,
        SelectorQuotaWindowStatus::Stale => QuotaWindowStatus::Stale,
        SelectorQuotaWindowStatus::Unknown => QuotaWindowStatus::Unknown,
        SelectorQuotaWindowStatus::Ineligible => QuotaWindowStatus::Ineligible,
    }
}

pub(in crate::quota) fn quota_window_label(limit_window_seconds: u64) -> &'static str {
    match limit_window_seconds {
        18_000 => "5h",
        86_400 => "daily",
        604_800 => "weekly",
        2_592_000 => "monthly",
        _ => "window",
    }
}
