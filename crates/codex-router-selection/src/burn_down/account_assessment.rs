//! Per-account quota, floor, and display assessment.
use super::assessment_input::BurnDownAccountInput;
use super::assessment_result::{
    AccountAvailability, BurnDownAccountAssessment, QuotaEvidenceFreshness, RoutingExclusion,
};
use super::credit_assessment;
use super::credit_assessment::credit_backed_candidate_is_eligible;
use super::routing_policy::{BurnDownRouteBandPolicy, DEFAULT_UNKNOWN_FALLBACK_WEIGHT, clamp_i64};
use super::routing_reasons::{QuotaEvidenceReason, RoutingReason};
use super::window_assessment::{
    AccountDisplayMetrics, WindowAssessment, assess_window, claude_near_full_reserve,
    freshness_for_windows, is_short_window, limiting_window, long_window_requires_reserve,
    missing_profile_window, missing_required_weekly_window, projected_runway_seconds,
    required_active_connections_to_drain, salvage_sort_key, short_window_fails_guard,
    weekly_window_is_drain_pool_candidate, weekly_window_survives_to_reset,
};
use super::window_facts::QuotaWindowStatus;
use super::{MAX_WEEKLY_QUOTA_FLOOR_BASIS_POINTS, V1_WEEKLY_WINDOW_SECONDS};
use crate::run_rate::QuotaRunRateConfidence;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::safe_account_label;
use codex_router_core::route_profile::RouteProfile;
use codex_router_core::routes::RouteBand;

pub(super) fn assess_account(
    input: &BurnDownAccountInput,
    now_unix_seconds: u64,
    policy: BurnDownRouteBandPolicy,
    route_profile: &RouteProfile,
    route_band: RouteBand,
) -> BurnDownAccountAssessment {
    let base = BurnDownAccountAssessment {
        account_id: input.account_id.clone(),
        account_label: safe_account_label(&input.account_label, &input.account_id)
            .as_str()
            .to_owned(),
        availability: AccountAvailability::Unknown,
        freshness: QuotaEvidenceFreshness::Unknown,
        routing_exclusion: RoutingExclusion::None,
        limiting_window: None,
        quota_evidence_reason: QuotaEvidenceReason::NeedsQuotaProbe,
        short_pressure: 0,
        long_pressure: 0,
        short_salvage: 0,
        long_salvage: 0,
        projected_burn_pressure: 0,
        routing_weight: Some(DEFAULT_UNKNOWN_FALLBACK_WEIGHT),
        routing_reason: RoutingReason::UnknownFallbackAvailable,
        weekly_floor_switch_band: false,
        credit_backed: false,
        preferred_next: false,
        far_idle_priority: false,
        current_active_sessions: input.current_active_sessions,
        weekly_reset_unix_seconds: None,
        weekly_projected_exhaustion_unix_seconds: None,
        weekly_survives_to_reset: false,
        weekly_survival_margin_basis_points: None,
        weekly_burn_rate_confidence: QuotaRunRateConfidence::Unknown,
        weekly_in_drain_pool: false,
        required_active_connections_to_drain: None,
        projected_drain_gap_after_selection: None,
        projected_weekly_runway_seconds: None,
        weekly_remaining_headroom: None,
        weekly_remaining_basis_points: None,
        weekly_projected_candidate_burn_basis_points_per_hour: None,
        initial_admission_priority: false,
        salvage_sort_key: None,
    };

    if !input.account_enabled {
        return BurnDownAccountAssessment {
            availability: AccountAvailability::Excluded,
            routing_exclusion: RoutingExclusion::Disabled,
            quota_evidence_reason: QuotaEvidenceReason::AccountDisabled,
            routing_reason: RoutingReason::ExcludedDisabled,
            routing_weight: None,
            ..base
        };
    }
    if !input.has_active_credential {
        return BurnDownAccountAssessment {
            availability: AccountAvailability::Excluded,
            routing_exclusion: RoutingExclusion::MissingCredential,
            quota_evidence_reason: QuotaEvidenceReason::MissingCredential,
            routing_reason: RoutingReason::ExcludedMissingCredential,
            routing_weight: None,
            ..base
        };
    }

    let own_windows = input
        .windows
        .iter()
        .map(|window| assess_window(window, now_unix_seconds, policy))
        .collect::<Vec<_>>();
    let canonical_compact_windows =
        credit_assessment::canonical_responses_compact_window_assessments(
            input,
            route_band,
            &own_windows,
            now_unix_seconds,
            policy,
        );
    let windows = canonical_compact_windows.as_deref().unwrap_or(&own_windows);
    let has_eligible_credit_backing = credit_backed_candidate_is_eligible(
        input,
        route_band,
        windows,
        canonical_compact_windows.is_some(),
    );
    if input.provider == Provider::Claude && !input.rejected_windows.is_empty() {
        return with_display_metrics(
            BurnDownAccountAssessment {
                availability: AccountAvailability::Blocked,
                freshness: freshness_for_windows(windows),
                limiting_window: limiting_window(windows),
                quota_evidence_reason: QuotaEvidenceReason::WindowExhausted,
                routing_reason: RoutingReason::BlockedWindowExhausted,
                routing_weight: None,
                ..base
            },
            account_display_metrics(input, windows, policy),
        );
    }
    if (windows.is_empty() || missing_required_weekly_window(windows))
        && input.weekly_quota_floor_basis_points.is_some()
    {
        let display_metrics = account_display_metrics(input, windows, policy);
        if weekly_quota_floor_excludes(input, windows) {
            return weekly_quota_floor_exclusion(base, windows, display_metrics);
        }
    }
    if input.provider == Provider::Claude
        && missing_profile_window(windows, route_profile.windows.as_ref())
    {
        return with_display_metrics(
            BurnDownAccountAssessment {
                limiting_window: limiting_window(windows),
                quota_evidence_reason: QuotaEvidenceReason::MissingExpectedWindow,
                ..base
            },
            account_display_metrics(input, windows, policy),
        );
    }
    if windows.is_empty() {
        return base;
    }
    if missing_required_weekly_window(windows) {
        return BurnDownAccountAssessment {
            limiting_window: limiting_window(windows),
            quota_evidence_reason: QuotaEvidenceReason::MissingExpectedWindow,
            ..base
        };
    }
    let display_metrics = account_display_metrics(input, windows, policy);
    if windows
        .iter()
        .any(|window| window.status == QuotaWindowStatus::Ineligible)
        && !has_eligible_credit_backing
    {
        return with_display_metrics(
            BurnDownAccountAssessment {
                availability: AccountAvailability::Blocked,
                freshness: freshness_for_windows(windows),
                limiting_window: limiting_window(windows),
                quota_evidence_reason: QuotaEvidenceReason::WindowIneligible,
                routing_reason: RoutingReason::BlockedWindowIneligible,
                routing_weight: None,
                ..base
            },
            display_metrics,
        );
    }
    if input.weekly_quota_floor_basis_points.is_some()
        && windows
            .iter()
            .any(|window| window.remaining_basis_points == 0)
    {
        return with_display_metrics(
            BurnDownAccountAssessment {
                availability: AccountAvailability::Blocked,
                freshness: freshness_for_windows(windows),
                limiting_window: limiting_window(windows),
                quota_evidence_reason: QuotaEvidenceReason::WindowExhausted,
                routing_reason: RoutingReason::BlockedWindowExhausted,
                routing_weight: None,
                ..base
            },
            display_metrics,
        );
    }
    if input.weekly_quota_floor_basis_points.is_some()
        && weekly_quota_floor_excludes(input, windows)
    {
        return weekly_quota_floor_exclusion(base, windows, display_metrics);
    }
    if windows
        .iter()
        .any(|window| window.status == QuotaWindowStatus::Unknown)
    {
        return with_display_metrics(
            BurnDownAccountAssessment {
                limiting_window: limiting_window(windows),
                quota_evidence_reason: QuotaEvidenceReason::UnknownQuotaWindow,
                ..base
            },
            display_metrics,
        );
    }
    if input.provider == Provider::Claude
        && windows
            .iter()
            .any(|window| window.status == QuotaWindowStatus::Stale)
    {
        return with_display_metrics(
            BurnDownAccountAssessment {
                freshness: QuotaEvidenceFreshness::Stale,
                limiting_window: limiting_window(windows),
                quota_evidence_reason: QuotaEvidenceReason::UnknownQuotaWindow,
                routing_reason: RoutingReason::UnknownFallbackAvailable,
                routing_weight: Some(DEFAULT_UNKNOWN_FALLBACK_WEIGHT),
                ..base
            },
            display_metrics,
        );
    }
    if windows
        .iter()
        .any(|window| window.remaining_basis_points == 0)
    {
        if has_eligible_credit_backing {
            return with_display_metrics(
                BurnDownAccountAssessment {
                    availability: AccountAvailability::Reserve,
                    freshness: QuotaEvidenceFreshness::Fresh,
                    limiting_window: limiting_window(windows),
                    quota_evidence_reason: QuotaEvidenceReason::CreditBacked,
                    routing_reason: RoutingReason::CreditBacked,
                    routing_weight: Some(0),
                    credit_backed: true,
                    ..base
                },
                display_metrics,
            );
        }
        return with_display_metrics(
            BurnDownAccountAssessment {
                availability: AccountAvailability::Blocked,
                freshness: freshness_for_windows(windows),
                limiting_window: limiting_window(windows),
                quota_evidence_reason: QuotaEvidenceReason::WindowExhausted,
                routing_reason: RoutingReason::BlockedWindowExhausted,
                routing_weight: None,
                ..base
            },
            display_metrics,
        );
    }
    if windows
        .iter()
        .any(|window| window.reset_unix_seconds.is_none())
    {
        return with_display_metrics(
            BurnDownAccountAssessment {
                limiting_window: limiting_window(windows),
                quota_evidence_reason: QuotaEvidenceReason::MissingResetTime,
                ..base
            },
            display_metrics,
        );
    }

    let usable_headroom = windows
        .iter()
        .map(|window| window.remaining_headroom)
        .min()
        .unwrap_or(0);
    let risk_penalty = policy.risk_penalty_cap.min(
        policy
            .long_pressure_multiplier
            .saturating_mul(display_metrics.long_pressure)
            .saturating_add(display_metrics.short_pressure),
    );
    let risk_adjusted_weight = i64::from(usable_headroom) - i64::from(risk_penalty)
        + i64::from(display_metrics.short_salvage)
        + i64::from(display_metrics.long_salvage);
    let routing_weight = clamp_i64(
        risk_adjusted_weight,
        policy.selectable_weight_min,
        policy.selectable_weight_max,
    );
    let availability =
        if claude_near_full_reserve(input.provider, windows, route_profile.windows.as_ref())
            || (input.provider == Provider::Openai && long_window_requires_reserve(windows, policy))
        {
            AccountAvailability::Reserve
        } else {
            AccountAvailability::Usable
        };
    if input.provider == Provider::Openai && short_window_fails_guard(windows, policy) {
        return with_display_metrics(
            BurnDownAccountAssessment {
                availability: AccountAvailability::Blocked,
                freshness: freshness_for_windows(windows),
                limiting_window: limiting_window(windows),
                quota_evidence_reason: QuotaEvidenceReason::ShortWindowGuard,
                routing_reason: RoutingReason::HeldShortWindowGuard,
                routing_weight: None,
                ..base
            },
            display_metrics,
        );
    }

    with_display_metrics(
        BurnDownAccountAssessment {
            availability,
            freshness: freshness_for_windows(windows),
            limiting_window: limiting_window(windows),
            quota_evidence_reason: QuotaEvidenceReason::Ok,
            routing_weight: Some(routing_weight),
            routing_reason: RoutingReason::AvailableSamePool,
            ..base
        },
        display_metrics,
    )
}

fn weekly_quota_floor_exclusion(
    base: BurnDownAccountAssessment,
    windows: &[WindowAssessment],
    display_metrics: AccountDisplayMetrics,
) -> BurnDownAccountAssessment {
    with_display_metrics(
        BurnDownAccountAssessment {
            availability: AccountAvailability::Excluded,
            freshness: freshness_for_windows(windows),
            routing_exclusion: RoutingExclusion::WeeklyQuotaFloor,
            limiting_window: limiting_window(windows),
            quota_evidence_reason: QuotaEvidenceReason::WeeklyQuotaFloor,
            routing_reason: RoutingReason::ExcludedWeeklyQuotaFloor,
            routing_weight: None,
            ..base
        },
        display_metrics,
    )
}

fn weekly_quota_floor_excludes(input: &BurnDownAccountInput, windows: &[WindowAssessment]) -> bool {
    let Some(floor_basis_points) = input.weekly_quota_floor_basis_points else {
        return false;
    };
    if floor_basis_points > MAX_WEEKLY_QUOTA_FLOOR_BASIS_POINTS {
        return true;
    }
    let Some(weekly_window) = windows
        .iter()
        .find(|window| window.window_seconds == V1_WEEKLY_WINDOW_SECONDS)
    else {
        return true;
    };

    if weekly_window.status != QuotaWindowStatus::Eligible {
        return true;
    }

    let current_remaining_basis_points = weekly_window.remaining_basis_points;
    current_remaining_basis_points <= floor_basis_points
}

fn account_display_metrics(
    input: &BurnDownAccountInput,
    windows: &[WindowAssessment],
    policy: BurnDownRouteBandPolicy,
) -> AccountDisplayMetrics {
    let short_pressure = windows
        .iter()
        .filter(|window| is_short_window(window.window_seconds, policy))
        .map(|window| window.pressure)
        .max()
        .unwrap_or(0);
    let long_pressure = windows
        .iter()
        .filter(|window| !is_short_window(window.window_seconds, policy))
        .map(|window| window.pressure)
        .max()
        .unwrap_or(0);
    let short_salvage = windows
        .iter()
        .filter(|window| is_short_window(window.window_seconds, policy) && window.near_reset)
        .map(|window| window.surplus)
        .max()
        .unwrap_or(0)
        .min(policy.short_salvage_cap);
    let long_salvage = windows
        .iter()
        .filter(|window| !is_short_window(window.window_seconds, policy) && window.near_reset)
        .map(|window| window.surplus)
        .max()
        .unwrap_or(0)
        .min(policy.long_salvage_cap);
    let projected_burn_pressure = windows
        .iter()
        .map(|window| window.projected_pressure)
        .max()
        .unwrap_or(0)
        .min(100);
    let weekly_window = windows
        .iter()
        .find(|window| !is_short_window(window.window_seconds, policy));
    let weekly_in_drain_pool = weekly_window.is_some_and(weekly_window_is_drain_pool_candidate);
    let required_active_connections_to_drain =
        weekly_window.and_then(required_active_connections_to_drain);
    let projected_drain_gap_after_selection =
        required_active_connections_to_drain.map(|required_active_connections_to_drain| {
            i64::from(required_active_connections_to_drain)
                - i64::from(input.current_active_sessions.saturating_add(1))
        });

    AccountDisplayMetrics {
        short_pressure,
        long_pressure,
        short_salvage,
        long_salvage,
        projected_burn_pressure,
        current_active_sessions: input.current_active_sessions,
        weekly_reset_unix_seconds: weekly_window.and_then(|window| window.reset_unix_seconds),
        weekly_projected_exhaustion_unix_seconds: weekly_window
            .and_then(|window| window.projected_exhaustion_unix_seconds),
        weekly_survives_to_reset: weekly_window.is_some_and(weekly_window_survives_to_reset),
        weekly_survival_margin_basis_points: weekly_window
            .and_then(|window| window.survival_margin_basis_points),
        weekly_burn_rate_confidence: weekly_window
            .map_or(QuotaRunRateConfidence::Unknown, |window| {
                window.burn_rate_confidence
            }),
        weekly_in_drain_pool,
        required_active_connections_to_drain,
        projected_drain_gap_after_selection,
        projected_weekly_runway_seconds: weekly_window.and_then(projected_runway_seconds),
        weekly_remaining_headroom: weekly_window.map(|window| window.remaining_headroom),
        weekly_remaining_basis_points: weekly_window.map(|window| window.remaining_basis_points),
        weekly_projected_candidate_burn_basis_points_per_hour: weekly_window
            .and_then(|window| window.projected_candidate_burn_basis_points_per_hour),
        salvage_sort_key: salvage_sort_key(windows, short_salvage, long_salvage, policy),
    }
}

fn with_display_metrics(
    mut assessment: BurnDownAccountAssessment,
    metrics: AccountDisplayMetrics,
) -> BurnDownAccountAssessment {
    assessment.short_pressure = metrics.short_pressure;
    assessment.long_pressure = metrics.long_pressure;
    assessment.short_salvage = metrics.short_salvage;
    assessment.long_salvage = metrics.long_salvage;
    assessment.projected_burn_pressure = metrics.projected_burn_pressure;
    assessment.current_active_sessions = metrics.current_active_sessions;
    assessment.weekly_reset_unix_seconds = metrics.weekly_reset_unix_seconds;
    assessment.weekly_projected_exhaustion_unix_seconds =
        metrics.weekly_projected_exhaustion_unix_seconds;
    assessment.weekly_survives_to_reset = metrics.weekly_survives_to_reset;
    assessment.weekly_survival_margin_basis_points = metrics.weekly_survival_margin_basis_points;
    assessment.weekly_burn_rate_confidence = metrics.weekly_burn_rate_confidence;
    assessment.weekly_in_drain_pool = metrics.weekly_in_drain_pool;
    assessment.required_active_connections_to_drain = metrics.required_active_connections_to_drain;
    assessment.projected_drain_gap_after_selection = metrics.projected_drain_gap_after_selection;
    assessment.projected_weekly_runway_seconds = metrics.projected_weekly_runway_seconds;
    assessment.weekly_remaining_headroom = metrics.weekly_remaining_headroom;
    assessment.weekly_remaining_basis_points = metrics.weekly_remaining_basis_points;
    assessment.weekly_projected_candidate_burn_basis_points_per_hour =
        metrics.weekly_projected_candidate_burn_basis_points_per_hour;
    assessment.salvage_sort_key = metrics.salvage_sort_key;
    assessment
}
