//! Route-level account assessment composition and candidate-pool selection.
use super::account_assessment::assess_account;
use super::assessment_input::BurnDownRouteBandAssessmentInput;
use super::assessment_result::{
    AccountAvailability, BurnDownAccountAssessment, BurnDownRouteBandAssessmentResult,
    QuotaEvidenceFreshness, RouteBandAssessmentStatus, RoutingExclusion, SelectedPool,
};
use super::candidate_priority::{candidate_priority_cmp, confidence_rank};
use super::routing_policy::{
    BurnDownRouteBandPolicy, clamp_u32, weekly_floor_switch_at_basis_points,
};
use super::routing_reasons::{
    QuotaEvidenceReason, RoutingReason, RoutingReasonContext, routing_reason_for_account,
};
use super::{DRAIN_POOL_RESET_HORIZON_SECONDS, REACTIVE_RECONNECT_MIN_RUNWAY_SECONDS};
use crate::run_rate::QuotaRunRateConfidence;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::{RouteProfile, WindowRule};

/// Assesses a route band.
#[must_use]
pub fn assess_route_band(
    input: BurnDownRouteBandAssessmentInput,
) -> BurnDownRouteBandAssessmentResult {
    let legacy_openai_profile = is_legacy_openai_profile(&input.route_profile);
    let mut accounts = input
        .accounts
        .iter()
        .filter(|account| account.provider == input.route_profile.provider)
        .map(|account| {
            let mut assessment = assess_account(
                account,
                input.now_unix_seconds,
                input.policy,
                &input.route_profile,
                input.route_band,
            );
            assessment.weekly_floor_switch_band = account
                .weekly_quota_floor_basis_points
                .zip(assessment.weekly_remaining_basis_points)
                .is_some_and(|(floor, remaining_basis_points)| {
                    assessment.routing_exclusion == RoutingExclusion::None
                        && assessment.quota_evidence_reason == QuotaEvidenceReason::Ok
                        && assessment.freshness == QuotaEvidenceFreshness::Fresh
                        && matches!(
                            assessment.availability,
                            AccountAvailability::Usable | AccountAvailability::Reserve
                        )
                        && remaining_basis_points > floor
                        && weekly_floor_switch_at_basis_points(floor, &input.route_profile)
                            .is_some_and(|switch_at| {
                                assessment
                                    .weekly_remaining_basis_points
                                    .is_some_and(|remaining| remaining <= switch_at)
                            })
                });
            if input.route_profile.provider == Provider::Claude
                && assessment.weekly_floor_switch_band
                && assessment.availability == AccountAvailability::Usable
            {
                assessment.availability = AccountAvailability::Reserve;
            }
            assessment
        })
        .collect::<Vec<_>>();
    accounts.sort_by(|left, right| left.account_id.cmp(&right.account_id));
    hold_credit_backing_behind_included_quota(&mut accounts);
    let healthy_peer_preference = accounts
        .iter()
        .any(|account| account.weekly_floor_switch_band)
        && accounts
            .iter()
            .any(BurnDownAccountAssessment::is_healthy_floor_switch_peer);
    let all_account_assessments = healthy_peer_preference.then(|| accounts.clone());
    if healthy_peer_preference {
        accounts.retain(BurnDownAccountAssessment::is_healthy_floor_switch_peer);
    }
    let ordinary_routing_weights = accounts
        .iter()
        .map(|account| (account.account_id.clone(), account.routing_weight))
        .collect::<Vec<_>>();
    if legacy_openai_profile {
        apply_initial_admission(
            &mut accounts,
            &ordinary_routing_weights,
            input.now_unix_seconds,
        );
        apply_far_idle_priority(&mut accounts, input.now_unix_seconds);
    }

    let selected_pool = if accounts.iter().any(|account| {
        account.availability == AccountAvailability::Usable && account.routing_weight.is_some()
    }) {
        SelectedPool::Usable
    } else if accounts.iter().any(|account| {
        account.availability == AccountAvailability::Reserve && account.routing_weight.is_some()
    }) {
        SelectedPool::Reserve
    } else if accounts.iter().any(|account| {
        account.availability == AccountAvailability::Unknown && account.routing_weight.is_some()
    }) {
        SelectedPool::Unknown
    } else if legacy_openai_profile
        && accounts.iter().any(|account| {
            account.routing_exclusion == RoutingExclusion::None
                && account.quota_evidence_reason == QuotaEvidenceReason::ShortWindowGuard
        })
    {
        SelectedPool::LastResort
    } else {
        SelectedPool::None
    };

    let has_fresh_account_in_selected_pool = accounts.iter().any(|account| {
        account_matches_selected_pool(account, selected_pool)
            && account.routing_weight.is_some()
            && account.freshness == QuotaEvidenceFreshness::Fresh
    });

    for account in &mut accounts {
        if selected_pool == SelectedPool::LastResort
            && account.quota_evidence_reason == QuotaEvidenceReason::ShortWindowGuard
        {
            account.routing_weight = Some(0);
        } else if account_matches_selected_pool(account, selected_pool)
            && let Some(weight) = account.routing_weight
        {
            let weight = selected_pool_weight(
                weight,
                account.freshness,
                has_fresh_account_in_selected_pool,
                input.policy,
            );
            account.routing_weight = Some(weight);
        }
    }

    let mut candidate_accounts = accounts
        .iter()
        .filter(|account| account_matches_selected_pool(account, selected_pool))
        .filter_map(|account| account.routing_weight.map(|weight| (account, weight)))
        .collect::<Vec<_>>();

    candidate_accounts.sort_by(|(left, left_weight), (right, right_weight)| {
        candidate_priority_cmp(left, *left_weight, right, *right_weight)
    });

    let weighted_candidates = candidate_accounts
        .iter()
        .map(|(account, weight)| (account.account_id.clone(), *weight))
        .collect::<Vec<_>>();
    let preferred_next = weighted_candidates
        .first()
        .map(|(account_id, _weight)| account_id.clone());
    if let Some(preferred_next) = &preferred_next {
        for account in &mut accounts {
            account.preferred_next = &account.account_id == preferred_next;
        }
    }
    let reason_context = RoutingReasonContext::from_accounts(&accounts, selected_pool);
    for account in &mut accounts {
        account.routing_reason = routing_reason_for_account(account, reason_context);
    }

    if let Some(mut all_accounts) = all_account_assessments {
        for account in &mut all_accounts {
            if let Some(selected) = accounts
                .iter()
                .find(|selected| selected.account_id == account.account_id)
            {
                *account = selected.clone();
            } else if account.routing_exclusion == RoutingExclusion::None
                && account.routing_reason != RoutingReason::HeldForIncludedQuota
                && matches!(
                    account.availability,
                    AccountAvailability::Usable | AccountAvailability::Reserve
                )
            {
                account.routing_reason = RoutingReason::HeldFloorSwitch;
            }
        }
        accounts = all_accounts;
    }

    BurnDownRouteBandAssessmentResult {
        route_band: input.route_band,
        route_status: RouteBandAssessmentStatus::Supported,
        accounts,
        selected_pool,
        weighted_candidates,
        preferred_next,
    }
}

fn hold_credit_backing_behind_included_quota(accounts: &mut [BurnDownAccountAssessment]) {
    let has_selectable_included_quota = accounts.iter().any(|account| {
        if account.credit_backed || account.routing_exclusion != RoutingExclusion::None {
            return false;
        }

        (matches!(
            account.availability,
            AccountAvailability::Usable
                | AccountAvailability::Reserve
                | AccountAvailability::Unknown
        ) && account.routing_weight.is_some())
            || account.quota_evidence_reason == QuotaEvidenceReason::ShortWindowGuard
    });
    if !has_selectable_included_quota {
        return;
    }

    for account in accounts.iter_mut().filter(|account| account.credit_backed) {
        account.routing_weight = None;
        account.routing_reason = RoutingReason::HeldForIncludedQuota;
        account.preferred_next = false;
    }
}

fn apply_initial_admission(
    accounts: &mut [BurnDownAccountAssessment],
    ordinary_routing_weights: &[(AccountId, Option<u32>)],
    now_unix_seconds: u64,
) {
    for account in accounts {
        let reset_qualifies = account.weekly_reset_unix_seconds.is_some_and(|reset| {
            reset > now_unix_seconds
                && reset.saturating_sub(now_unix_seconds) <= DRAIN_POOL_RESET_HORIZON_SECONDS
        });
        let burn_evidence_qualifies =
            matches!(
                account.weekly_burn_rate_confidence,
                QuotaRunRateConfidence::Unknown
                    | QuotaRunRateConfidence::Insufficient
                    | QuotaRunRateConfidence::Stale
            ) || account.weekly_projected_candidate_burn_basis_points_per_hour == Some(0);
        let evidence_qualifies = account.quota_evidence_reason == QuotaEvidenceReason::Ok
            && account.freshness == QuotaEvidenceFreshness::Fresh
            && account
                .weekly_remaining_headroom
                .is_some_and(|remaining| remaining > 0)
            && account.projected_weekly_runway_seconds.is_none();
        let evidence_qualifies = evidence_qualifies && burn_evidence_qualifies;
        let metadata_qualifies = account.routing_exclusion == RoutingExclusion::None
            && matches!(
                account.availability,
                AccountAvailability::Usable | AccountAvailability::Reserve
            );

        if account.current_active_sessions != 0
            || !reset_qualifies
            || !evidence_qualifies
            || !metadata_qualifies
        {
            continue;
        }

        let ordinary_weight = ordinary_routing_weights
            .iter()
            .find(|(account_id, _)| account_id == &account.account_id)
            .and_then(|(_, weight)| *weight);
        let Some(ordinary_weight) = ordinary_weight else {
            continue;
        };

        account.availability = AccountAvailability::Usable;
        account.routing_weight = Some(ordinary_weight);
        account.initial_admission_priority = true;
    }
}

fn apply_far_idle_priority(accounts: &mut [BurnDownAccountAssessment], now_unix_seconds: u64) {
    if accounts.iter().any(|account| {
        (account.initial_admission_priority || account.weekly_in_drain_pool)
            && account.routing_weight.is_some()
            && matches!(
                account.availability,
                AccountAvailability::Usable | AccountAvailability::Reserve
            )
    }) {
        return;
    }
    let preferred_index = accounts
        .iter()
        .enumerate()
        .filter(|(_, account)| {
            account.current_active_sessions == 0
                && account.routing_exclusion == RoutingExclusion::None
                && account.quota_evidence_reason == QuotaEvidenceReason::Ok
                && account.freshness == QuotaEvidenceFreshness::Fresh
                && matches!(
                    account.availability,
                    AccountAvailability::Usable | AccountAvailability::Reserve
                )
                && account.routing_weight.is_some()
                && account
                    .weekly_remaining_headroom
                    .is_some_and(|remaining| remaining > 0)
                && account.weekly_reset_unix_seconds.is_some_and(|reset| {
                    reset > now_unix_seconds.saturating_add(DRAIN_POOL_RESET_HORIZON_SECONDS)
                })
                && match account.projected_weekly_runway_seconds {
                    Some(runway) => {
                        runway >= REACTIVE_RECONNECT_MIN_RUNWAY_SECONDS
                            && matches!(
                                account.weekly_burn_rate_confidence,
                                QuotaRunRateConfidence::Normal | QuotaRunRateConfidence::Low
                            )
                            && account
                                .weekly_projected_candidate_burn_basis_points_per_hour
                                .is_some_and(|burn| burn > 0)
                    }
                    None => {
                        matches!(
                            account.weekly_burn_rate_confidence,
                            QuotaRunRateConfidence::Unknown
                                | QuotaRunRateConfidence::Insufficient
                                | QuotaRunRateConfidence::Stale
                        ) || account.weekly_projected_candidate_burn_basis_points_per_hour
                            == Some(0)
                    }
                }
        })
        .min_by(|(_, left), (_, right)| {
            left.weekly_reset_unix_seconds
                .cmp(&right.weekly_reset_unix_seconds)
                .then_with(|| {
                    left.weekly_remaining_headroom
                        .cmp(&right.weekly_remaining_headroom)
                })
                .then_with(|| {
                    confidence_rank(right.weekly_burn_rate_confidence)
                        .cmp(&confidence_rank(left.weekly_burn_rate_confidence))
                })
                .then_with(|| left.account_id.cmp(&right.account_id))
        })
        .map(|(index, _)| index);
    if let Some(account) = preferred_index.and_then(|index| accounts.get_mut(index)) {
        account.availability = AccountAvailability::Usable;
        account.far_idle_priority = true;
    }
}

fn selected_pool_matches(selected_pool: SelectedPool, availability: AccountAvailability) -> bool {
    matches!(
        (selected_pool, availability),
        (SelectedPool::Usable, AccountAvailability::Usable)
            | (SelectedPool::Reserve, AccountAvailability::Reserve)
            | (SelectedPool::Unknown, AccountAvailability::Unknown)
    )
}

pub(super) fn account_matches_selected_pool(
    account: &BurnDownAccountAssessment,
    selected_pool: SelectedPool,
) -> bool {
    if selected_pool == SelectedPool::LastResort {
        return account.quota_evidence_reason == QuotaEvidenceReason::ShortWindowGuard;
    }

    selected_pool_matches(selected_pool, account.availability)
}

fn selected_pool_weight(
    weight: u32,
    freshness: QuotaEvidenceFreshness,
    has_fresh_account_in_selected_pool: bool,
    policy: BurnDownRouteBandPolicy,
) -> u32 {
    let adjusted =
        if freshness == QuotaEvidenceFreshness::Stale && has_fresh_account_in_selected_pool {
            weight / 4
        } else {
            weight
        };

    clamp_u32(
        adjusted,
        policy.selectable_weight_min,
        policy.selectable_weight_max,
    )
}

fn is_legacy_openai_profile(route_profile: &RouteProfile) -> bool {
    route_profile.provider == Provider::Openai
        && !route_profile.windows.is_empty()
        && route_profile
            .windows
            .iter()
            .all(|window_policy| window_policy.rule == WindowRule::LegacyOpenAi)
}
