use super::*;

pub(super) struct QuotaStatusReport {
    pub(super) app_version: String,
    pub(super) route_band: String,
    pub(super) selected_pool: SelectedPool,
    pub(super) preferred_next_account_id: Option<AccountId>,
    pub(super) selection_projection_source: SelectionProjectionSource,
    pub(super) now_unix_seconds: u64,
    pub(super) rows: Vec<QuotaStatusRow>,
}

impl QuotaStatusReport {
    pub(super) fn rows(&self) -> &[QuotaStatusRow] {
        &self.rows
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectionProjectionSource {
    SqlxProjection,
    DisplayWindowsFallback,
}

impl SelectionProjectionSource {
    pub(super) const fn as_json(self) -> &'static str {
        match self {
            Self::SqlxProjection => "sqlx_projection",
            Self::DisplayWindowsFallback => "display_windows_fallback",
        }
    }

    pub(super) const fn route_result(self) -> &'static str {
        match self {
            Self::SqlxProjection => "ok",
            Self::DisplayWindowsFallback => "degraded",
        }
    }

    pub(super) const fn is_authoritative(self) -> bool {
        matches!(self, Self::SqlxProjection)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct QuotaStatusAccountInput {
    pub(super) account_label: String,
    pub(super) account_status: String,
    pub(super) account_id: AccountId,
    pub(super) active_credential_generation: Option<u64>,
    pub(super) reset_credits_available: Option<u32>,
    pub(super) updated: String,
    pub(super) active_clients: ActiveClientMirrorStatus,
    pub(super) windows: Vec<DisplayQuotaWindow>,
    pub(super) weekly_pace: Option<QuotaPaceSnapshot>,
    pub(super) weekly_quota_floor_basis_points: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct QuotaStatusRow {
    pub(super) account_id: AccountId,
    pub(super) active_credential_generation: Option<u64>,
    pub(super) account_label: String,
    pub(super) account_status: String,
    pub(super) short_window: String,
    pub(super) weekly_window: String,
    pub(super) pace: String,
    pub(super) burn: String,
    pub(super) updated: String,
    pub(super) active_clients: String,
    pub(super) active_clients_value: Option<u32>,
    pub(super) active_clients_source: &'static str,
    pub(super) reset_credits_available: String,
    pub(super) reset_credits_available_value: Option<u32>,
    pub(super) routing: String,
    pub(super) next_use: String,
    pub(super) weekly_pace: Option<QuotaPaceSnapshot>,
    pub(super) windows: Vec<DisplayQuotaWindow>,
    pub(super) availability: AccountAvailability,
    pub(super) freshness: QuotaEvidenceFreshness,
    pub(super) routing_exclusion: RoutingExclusion,
    pub(super) quota_evidence_reason: QuotaEvidenceReason,
    pub(super) routing_reason: RoutingReason,
    pub(super) preferred_next: bool,
    pub(super) short_pressure: u32,
    pub(super) long_pressure: u32,
    pub(super) short_salvage: u32,
    pub(super) long_salvage: u32,
    pub(super) limiting_window: Option<LimitingWindow>,
    pub(super) weekly_survival_margin_basis_points: Option<i64>,
    pub(super) weekly_projected_exhaustion_unix_seconds: Option<u64>,
    pub(super) weekly_burn_rate_confidence: QuotaRunRateConfidence,
    pub(super) weekly_quota_floor_basis_points: Option<u32>,
}

impl QuotaStatusRow {
    pub(super) fn from_assessment(
        input: &QuotaStatusAccountInput,
        assessment: &BurnDownAccountAssessment,
        now_unix_seconds: u64,
        unicode_bars: bool,
    ) -> Self {
        Self {
            account_id: input.account_id.clone(),
            active_credential_generation: input.active_credential_generation,
            account_label: assessment.account_label().to_owned(),
            account_status: input.account_status.clone(),
            short_window: format_window_cell(
                &input.windows,
                V1_SHORT_WINDOW_SECONDS,
                now_unix_seconds,
                unicode_bars,
            ),
            weekly_window: format_window_cell(
                &input.windows,
                V1_WEEKLY_WINDOW_SECONDS,
                now_unix_seconds,
                unicode_bars,
            ),
            pace: format_pace_cell(&input.windows, assessment, now_unix_seconds),
            burn: format_burn_cell(assessment),
            updated: input.updated.clone(),
            active_clients: format_active_clients(input.active_clients),
            active_clients_value: input.active_clients.count(),
            active_clients_source: input.active_clients.source(),
            reset_credits_available: format_reset_credits(input.reset_credits_available),
            reset_credits_available_value: input.reset_credits_available,
            routing: format_routing_cell(assessment),
            next_use: format_next_use(assessment).to_owned(),
            weekly_pace: input.weekly_pace,
            windows: input.windows.clone(),
            availability: assessment.availability(),
            freshness: assessment.freshness(),
            routing_exclusion: assessment.routing_exclusion(),
            quota_evidence_reason: assessment.quota_evidence_reason(),
            routing_reason: assessment.routing_reason(),
            preferred_next: assessment.preferred_next(),
            short_pressure: assessment.short_pressure(),
            long_pressure: assessment.long_pressure(),
            short_salvage: assessment.short_salvage(),
            long_salvage: assessment.long_salvage(),
            limiting_window: assessment.limiting_window(),
            weekly_survival_margin_basis_points: assessment.weekly_survival_margin_basis_points(),
            weekly_projected_exhaustion_unix_seconds: assessment
                .weekly_projected_exhaustion_unix_seconds(),
            weekly_burn_rate_confidence: assessment.weekly_burn_rate_confidence(),
            weekly_quota_floor_basis_points: input.weekly_quota_floor_basis_points,
        }
    }

    pub(super) fn normalize_degraded_projection_authority(&mut self) {
        self.preferred_next = false;
        if routing_reason_is_preferred(self.routing_reason) {
            self.routing_reason = RoutingReason::UnknownFallbackAvailable;
            self.routing = format_routing_reason(self.routing_reason).to_owned();
            self.next_use = format_next_use_from_routing_reason(self.routing_reason).to_owned();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct QuotaPaceSnapshot {
    pub(super) remaining_headroom: u32,
    pub(super) reset_unix_seconds: Option<u64>,
    pub(super) projected_exhaustion_unix_seconds: Option<u64>,
    pub(super) projected_candidate_burn_basis_points_per_hour: Option<u32>,
    pub(super) aggregate_burn_basis_points_per_hour: Option<u32>,
    pub(super) per_connection_burn_basis_points_per_hour: Option<u32>,
    pub(super) confidence: QuotaRunRateConfidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DisplayQuotaWindow {
    pub(super) window_seconds: u64,
    pub(super) status: QuotaWindowStatus,
    pub(super) remaining_headroom: u32,
    pub(super) reset_unix_seconds: Option<u64>,
    pub(super) observed_unix_seconds: u64,
    pub(super) effective: bool,
    pub(super) run_rate_estimate: QuotaRunRateEstimate,
}

impl DisplayQuotaWindow {
    pub(super) fn from_selector_window(window: &PersistedSelectorQuotaWindow) -> Self {
        Self {
            window_seconds: window.limit_window_seconds(),
            status: quota_window_status_from_selector_status(window.status()),
            remaining_headroom: window.remaining_headroom(),
            reset_unix_seconds: window.reset_unix_seconds(),
            observed_unix_seconds: window.observed_unix_seconds(),
            effective: window.effective(),
            run_rate_estimate: QuotaRunRateEstimate::unknown(),
        }
    }

    pub(super) fn from_snapshot(snapshot: &PersistedQuotaSnapshot) -> Self {
        Self {
            window_seconds: V1_SHORT_WINDOW_SECONDS,
            status: if snapshot.stale_penalty() {
                QuotaWindowStatus::Stale
            } else {
                QuotaWindowStatus::Eligible
            },
            remaining_headroom: snapshot.remaining_headroom(),
            reset_unix_seconds: snapshot.reset_unix_seconds(),
            observed_unix_seconds: snapshot.observed_unix_seconds(),
            effective: true,
            run_rate_estimate: QuotaRunRateEstimate::unknown(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ActiveClientMirrorStatus {
    MirrorFresh {
        count: u32,
        pressure: u32,
        max_age_seconds: u64,
    },
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ActiveClientMirrorLoad {
    pub(super) count: u32,
    pub(super) pressure: u32,
}

impl ActiveClientMirrorLoad {
    pub(super) const EMPTY: Self = Self {
        count: 0,
        pressure: 0,
    };
}

impl ActiveClientMirrorStatus {
    pub(super) const fn count(self) -> Option<u32> {
        match self {
            Self::MirrorFresh { count, .. } => Some(count),
            Self::Unavailable => None,
        }
    }

    pub(super) const fn source(self) -> &'static str {
        match self {
            Self::MirrorFresh { .. } => "sqlx_mirror",
            Self::Unavailable => "unavailable",
        }
    }
}
