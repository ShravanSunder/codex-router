use super::*;

#[derive(Serialize)]
pub(super) struct JsonQuotaStatusReport {
    pub(super) route_result: &'static str,
    pub(super) app_version: String,
    pub(super) route_band: String,
    pub(super) selection_projection_source: &'static str,
    pub(super) selected_pool: &'static str,
    pub(super) selected_pool_reason: &'static str,
    pub(super) preferred_next_account_hash: Option<String>,
    pub(super) accounts: Vec<JsonQuotaStatusAccount>,
}

impl JsonQuotaStatusReport {
    pub(super) fn from_report(report: &QuotaStatusReport) -> Self {
        Self {
            route_result: report.selection_projection_source.route_result(),
            app_version: report.app_version.clone(),
            route_band: report.route_band.clone(),
            selection_projection_source: report.selection_projection_source.as_json(),
            selected_pool: selected_pool_json(report.selected_pool),
            selected_pool_reason: selected_pool_reason_json(report.selected_pool),
            preferred_next_account_hash: report
                .preferred_next_account_id
                .as_ref()
                .map(|account_id| telemetry_hash(account_id.as_str())),
            accounts: report
                .rows
                .iter()
                .map(|row| JsonQuotaStatusAccount::from_row(row, report.now_unix_seconds))
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct JsonQuotaStatusAccount {
    pub(super) account_hash: String,
    pub(super) safe_account_label: String,
    pub(super) availability: &'static str,
    pub(super) freshness: &'static str,
    pub(super) routing_exclusion: &'static str,
    pub(super) next_use: String,
    pub(super) limiting_window: &'static str,
    pub(super) quota_evidence_reason: &'static str,
    pub(super) short_quota_guard: Option<u32>,
    pub(super) weekly_quota_guard: Option<u32>,
    pub(super) weekly_survival_margin_basis_points: Option<i64>,
    pub(super) weekly_projected_exhaustion_unix_seconds: Option<u64>,
    pub(super) short_guard_result: &'static str,
    pub(super) current_active_sessions: Option<u32>,
    pub(super) active_session_source: &'static str,
    pub(super) weekly_burn_rate_confidence: &'static str,
    pub(super) hard_block_reason: Option<&'static str>,
    pub(super) short_salvage: Option<u32>,
    pub(super) long_salvage: Option<u32>,
    pub(super) salvage_tie_key: Option<JsonSalvageTieKey>,
    pub(super) routing_reason: &'static str,
    pub(super) preferred_next: bool,
    pub(super) reset_credits_available: Option<u32>,
    pub(super) active_clients: Option<u32>,
    pub(super) active_clients_source: &'static str,
    pub(super) updated: String,
    pub(super) window_slots: JsonWindowSlots,
    pub(super) windows: Vec<JsonQuotaWindow>,
}

impl JsonQuotaStatusAccount {
    pub(super) fn from_row(row: &QuotaStatusRow, now_unix_seconds: u64) -> Self {
        Self {
            account_hash: telemetry_hash(row.account_id.as_str()),
            safe_account_label: row.account_label.clone(),
            availability: availability_json(row.availability),
            freshness: freshness_json(row.freshness),
            routing_exclusion: routing_exclusion_json(row.routing_exclusion),
            next_use: row.next_use.clone(),
            limiting_window: row
                .limiting_window
                .map_or("none", |window| quota_window_label(window.window_seconds())),
            quota_evidence_reason: quota_evidence_reason_json(row.quota_evidence_reason),
            short_quota_guard: Some(row.short_pressure),
            weekly_quota_guard: Some(row.long_pressure),
            weekly_survival_margin_basis_points: row.weekly_survival_margin_basis_points,
            weekly_projected_exhaustion_unix_seconds: row.weekly_projected_exhaustion_unix_seconds,
            short_guard_result: short_guard_result_json(row),
            current_active_sessions: row.active_clients_value,
            active_session_source: row.active_clients_source,
            weekly_burn_rate_confidence: run_rate_confidence_label(row.weekly_burn_rate_confidence),
            hard_block_reason: hard_block_reason_json(row),
            short_salvage: Some(row.short_salvage),
            long_salvage: Some(row.long_salvage),
            salvage_tie_key: None,
            routing_reason: routing_reason_json(row.routing_reason),
            preferred_next: row.preferred_next,
            reset_credits_available: row.reset_credits_available_value,
            active_clients: row.active_clients_value,
            active_clients_source: row.active_clients_source,
            updated: row.updated.clone(),
            window_slots: JsonWindowSlots::from_windows(&row.windows, now_unix_seconds),
            windows: row
                .windows
                .iter()
                .map(|window| JsonQuotaWindow::from_window(window, now_unix_seconds))
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct JsonSalvageTieKey {
    pub(super) reset_unix_seconds: u64,
    pub(super) window_seconds: u64,
}

#[derive(Serialize)]
pub(super) struct JsonWindowSlots {
    #[serde(rename = "5h")]
    pub(super) short: JsonWindowSlot,
    pub(super) weekly: JsonWindowSlot,
}

impl JsonWindowSlots {
    pub(super) fn from_windows(windows: &[DisplayQuotaWindow], now_unix_seconds: u64) -> Self {
        Self {
            short: JsonWindowSlot::from_windows(windows, V1_SHORT_WINDOW_SECONDS, now_unix_seconds),
            weekly: JsonWindowSlot::from_windows(
                windows,
                V1_WEEKLY_WINDOW_SECONDS,
                now_unix_seconds,
            ),
        }
    }
}

#[derive(Serialize)]
pub(super) struct JsonWindowSlot {
    pub(super) slot: &'static str,
    pub(super) evidence_state: &'static str,
    pub(super) remaining_headroom: Option<u32>,
    pub(super) reset_unix_seconds: Option<u64>,
    pub(super) reset_duration_seconds: Option<u64>,
    pub(super) display_note: String,
    pub(super) run_rate: JsonRunRateEstimate,
}

impl JsonWindowSlot {
    pub(super) fn from_windows(
        windows: &[DisplayQuotaWindow],
        window_seconds: u64,
        now_unix_seconds: u64,
    ) -> Self {
        let Some(window) = windows
            .iter()
            .find(|window| window.window_seconds == window_seconds)
        else {
            return Self {
                slot: quota_window_label(window_seconds),
                evidence_state: "no_data",
                remaining_headroom: None,
                reset_unix_seconds: None,
                reset_duration_seconds: None,
                display_note: "needs refresh".to_owned(),
                run_rate: JsonRunRateEstimate::unknown(),
            };
        };
        let reset_duration_seconds = window
            .reset_unix_seconds
            .map(|reset_unix_seconds| reset_unix_seconds.saturating_sub(now_unix_seconds));
        let display_note = window_display_note(window, now_unix_seconds);
        Self {
            slot: quota_window_label(window_seconds),
            evidence_state: window_evidence_state(window.status),
            remaining_headroom: window_known_headroom(window),
            reset_unix_seconds: window.reset_unix_seconds,
            reset_duration_seconds,
            display_note,
            run_rate: JsonRunRateEstimate::from_estimate(
                window.run_rate_estimate,
                now_unix_seconds,
            ),
        }
    }
}

#[derive(Serialize)]
pub(super) struct JsonRunRateEstimate {
    pub(super) confidence: &'static str,
    pub(super) burn_rate_percent_per_hour: Option<u32>,
    pub(super) burn_rate_basis_points_per_hour: Option<u32>,
    pub(super) projected_exhaustion_unix_seconds: Option<u64>,
}

impl JsonRunRateEstimate {
    pub(super) fn unknown() -> Self {
        Self {
            confidence: "unknown",
            burn_rate_percent_per_hour: None,
            burn_rate_basis_points_per_hour: None,
            projected_exhaustion_unix_seconds: None,
        }
    }

    pub(super) fn from_estimate(estimate: QuotaRunRateEstimate, now_unix_seconds: u64) -> Self {
        Self {
            confidence: run_rate_confidence_label(estimate.confidence()),
            burn_rate_percent_per_hour: estimate.burn_rate_percent_per_hour(),
            burn_rate_basis_points_per_hour: estimate.burn_rate_basis_points_per_hour(),
            projected_exhaustion_unix_seconds: estimate
                .projected_exhaustion_unix_seconds(now_unix_seconds),
        }
    }
}

#[derive(Serialize)]
pub(super) struct JsonQuotaWindow {
    pub(super) window_seconds: u64,
    pub(super) status: &'static str,
    pub(super) remaining_headroom: Option<u32>,
    pub(super) reset_unix_seconds: Option<u64>,
    pub(super) observed_unix_seconds: Option<u64>,
    pub(super) effective: bool,
    pub(super) guard_deficit_percent: Option<u32>,
    pub(super) surplus_percent: Option<u32>,
    pub(super) contributed_to_salvage: bool,
    pub(super) run_rate: JsonRunRateEstimate,
}

impl JsonQuotaWindow {
    pub(super) fn from_window(window: &DisplayQuotaWindow, now_unix_seconds: u64) -> Self {
        let (guard_deficit_percent, surplus_percent) =
            window_pressure_and_surplus(window, now_unix_seconds);
        Self {
            window_seconds: window.window_seconds,
            status: quota_window_status_json(window.status),
            remaining_headroom: window_known_headroom(window),
            reset_unix_seconds: window.reset_unix_seconds,
            observed_unix_seconds: Some(window.observed_unix_seconds),
            effective: window.effective,
            guard_deficit_percent,
            surplus_percent,
            contributed_to_salvage: surplus_percent.is_some_and(|surplus| surplus > 0),
            run_rate: JsonRunRateEstimate::from_estimate(
                window.run_rate_estimate,
                now_unix_seconds,
            ),
        }
    }
}

pub(super) const fn selected_pool_json(value: SelectedPool) -> &'static str {
    match value {
        SelectedPool::Usable => "usable",
        SelectedPool::Reserve => "reserve",
        SelectedPool::Unknown => "unknown",
        SelectedPool::LastResort => "last_resort",
        SelectedPool::None => "none",
    }
}

pub(super) const fn selected_pool_reason_json(value: SelectedPool) -> &'static str {
    match value {
        SelectedPool::Usable => "usable_available",
        SelectedPool::Reserve => "reserve_only",
        SelectedPool::Unknown => "unknown_fallback_only",
        SelectedPool::LastResort => "last_resort_5h_guard",
        SelectedPool::None => "none_available",
    }
}

pub(super) const fn availability_json(value: AccountAvailability) -> &'static str {
    match value {
        AccountAvailability::Usable => "usable",
        AccountAvailability::Reserve => "reserve",
        AccountAvailability::Retiring => "retiring",
        AccountAvailability::Blocked => "blocked",
        AccountAvailability::Unknown => "unknown",
        AccountAvailability::Excluded => "excluded",
    }
}

pub(super) const fn freshness_json(value: QuotaEvidenceFreshness) -> &'static str {
    match value {
        QuotaEvidenceFreshness::Fresh => "fresh",
        QuotaEvidenceFreshness::Stale => "stale",
        QuotaEvidenceFreshness::Unknown => "unknown",
    }
}

pub(super) const fn routing_exclusion_json(value: RoutingExclusion) -> &'static str {
    match value {
        RoutingExclusion::None => "none",
        RoutingExclusion::Disabled => "disabled",
        RoutingExclusion::MissingCredential => "missing_credential",
    }
}

pub(super) const fn quota_evidence_reason_json(value: QuotaEvidenceReason) -> &'static str {
    match value {
        QuotaEvidenceReason::Ok => "none",
        QuotaEvidenceReason::NeedsQuotaProbe => "needs_quota_refresh",
        QuotaEvidenceReason::MissingExpectedWindow => "missing_expected_window",
        QuotaEvidenceReason::WindowIneligible => "window_ineligible",
        QuotaEvidenceReason::WindowExhausted => "window_exhausted",
        QuotaEvidenceReason::UnknownQuotaWindow => "unknown_quota_window",
        QuotaEvidenceReason::MissingResetTime => "missing_reset_time",
        QuotaEvidenceReason::ShortWindowGuard => "short_window_guard",
        QuotaEvidenceReason::AccountDisabled => "account_disabled",
        QuotaEvidenceReason::MissingCredential => "missing_credential",
    }
}

pub(super) const fn routing_reason_json(value: RoutingReason) -> &'static str {
    value.as_str()
}

pub(super) fn short_guard_result_json(row: &QuotaStatusRow) -> &'static str {
    if row.routing_reason == RoutingReason::HeldShortWindowGuard
        || row.quota_evidence_reason == QuotaEvidenceReason::ShortWindowGuard
    {
        return "held";
    }
    let Some(short_window) = row
        .windows
        .iter()
        .find(|window| window.window_seconds == V1_SHORT_WINDOW_SECONDS)
    else {
        return "unknown";
    };
    match short_window.status {
        QuotaWindowStatus::Eligible | QuotaWindowStatus::Stale | QuotaWindowStatus::Ineligible => {
            "pass"
        }
        QuotaWindowStatus::Unknown => "unknown",
    }
}

pub(super) fn hard_block_reason_json(row: &QuotaStatusRow) -> Option<&'static str> {
    match row.quota_evidence_reason {
        QuotaEvidenceReason::Ok => None,
        reason => Some(quota_evidence_reason_json(reason)),
    }
}

pub(super) const fn quota_window_status_json(value: QuotaWindowStatus) -> &'static str {
    match value {
        QuotaWindowStatus::Eligible => "eligible",
        QuotaWindowStatus::Stale => "stale",
        QuotaWindowStatus::Unknown => "unknown",
        QuotaWindowStatus::Ineligible => "ineligible",
    }
}

pub(super) const fn window_evidence_state(value: QuotaWindowStatus) -> &'static str {
    match value {
        QuotaWindowStatus::Eligible | QuotaWindowStatus::Stale | QuotaWindowStatus::Ineligible => {
            "known"
        }
        QuotaWindowStatus::Unknown => "unknown",
    }
}

pub(super) fn window_known_headroom(window: &DisplayQuotaWindow) -> Option<u32> {
    match window.status {
        QuotaWindowStatus::Unknown => None,
        QuotaWindowStatus::Eligible | QuotaWindowStatus::Stale | QuotaWindowStatus::Ineligible => {
            Some(window.remaining_headroom)
        }
    }
}

pub(super) fn window_display_note(window: &DisplayQuotaWindow, now_unix_seconds: u64) -> String {
    let reset = window.reset_unix_seconds.map_or_else(
        || "reset unknown".to_owned(),
        |reset| format!("resets {}", format_relative_time(reset, now_unix_seconds)),
    );
    match window.status {
        QuotaWindowStatus::Eligible => reset,
        QuotaWindowStatus::Stale => reset,
        QuotaWindowStatus::Unknown => "unknown; needs refresh".to_owned(),
        QuotaWindowStatus::Ineligible if window.remaining_headroom == 0 => reset,
        QuotaWindowStatus::Ineligible => "quota ineligible".to_owned(),
    }
}

pub(super) fn window_pressure_and_surplus(
    window: &DisplayQuotaWindow,
    now_unix_seconds: u64,
) -> (Option<u32>, Option<u32>) {
    if window.status == QuotaWindowStatus::Unknown {
        return (None, None);
    }
    let Some(reset_unix_seconds) = window.reset_unix_seconds else {
        return (None, None);
    };
    let time_left_seconds = reset_unix_seconds
        .saturating_sub(now_unix_seconds)
        .min(window.window_seconds);
    let expected_remaining_percent = time_left_seconds
        .saturating_mul(100)
        .saturating_add(window.window_seconds.saturating_sub(1))
        / window.window_seconds;
    let expected_remaining_percent = u32::try_from(expected_remaining_percent)
        .unwrap_or(u32::MAX)
        .min(100);
    let remaining_headroom = window.remaining_headroom.min(100);

    (
        Some(expected_remaining_percent.saturating_sub(remaining_headroom)),
        Some(remaining_headroom.saturating_sub(expected_remaining_percent)),
    )
}
