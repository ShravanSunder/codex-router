use super::*;

/// Classifies the safe route-band action after one account reports quota exhaustion.
pub struct RouteBandPostExhaustionOutcomeInput<'a, TRepository> {
    /// Repository used to project persisted route-band selector state.
    pub state_repository: &'a TRepository,
    /// Process-local active reservations, when the caller owns them.
    pub active_reservations: Option<&'a RouteBandReservationBooks>,
    /// Process-local account exhaustion overlays, when available.
    pub runtime_exhaustions: Option<&'a RouteBandRuntimeExhaustions>,
    /// Process-local degraded write-queue health, when available.
    pub route_band_queue_health: Option<&'a RouteBandQueueHealth>,
    /// Route whose alternatives are being assessed.
    pub route_band: RouteBand,
    /// Provider policy that filters and assesses those alternatives.
    pub route_profile: RouteProfile,
    /// Account that reported the exhaustion event.
    pub excluded_account_id: &'a AccountId,
    /// Current selector time in Unix seconds.
    pub now_unix_seconds: u64,
}

/// Classifies the safe route-band action after one account reports quota exhaustion.
pub async fn route_band_post_exhaustion_outcome<TRepository>(
    input: RouteBandPostExhaustionOutcomeInput<'_, TRepository>,
) -> Result<PostExhaustionRouteBandOutcome, StateStoreError>
where
    TRepository: AsyncSelectionProjectionRepository + Sync,
{
    let RouteBandPostExhaustionOutcomeInput {
        state_repository,
        active_reservations,
        runtime_exhaustions,
        route_band_queue_health,
        route_band,
        route_profile,
        excluded_account_id,
        now_unix_seconds,
    } = input;
    if let Some(route_band_queue_health) = route_band_queue_health {
        route_band_queue_health_allows_selection(route_band_queue_health, route_band)?;
    }
    let active_session_overrides = match active_reservations {
        Some(active_reservations) => {
            let mut reservations =
                active_reservations
                    .lock()
                    .map_err(|_error| StateStoreError::Sqlite {
                        message: "active reservation state unavailable".to_owned(),
                    })?;
            if let Some(book) = reservations.get_mut(route_band.as_str()) {
                book.purge_stale(now_unix_seconds, ACTIVE_RESERVATION_MAX_AGE_SECONDS);
            }
            reservations
                .get(route_band.as_str())
                .map(active_session_counts_by_account)
        }
        None => None,
    };
    let projection = project_route_band_selection_inputs_with_active_counts_read_only(
        state_repository,
        route_band.as_str(),
        now_unix_seconds,
        ACTIVE_RESERVATION_MAX_AGE_SECONDS,
        active_session_overrides.as_ref(),
    )
    .await?;
    let provider_accounts = projection
        .accounts()
        .iter()
        .filter(|account| account.provider() == route_profile.provider)
        .collect::<Vec<_>>();
    let short_quota_wait_jitter_seconds = short_quota_wait_jitter_seconds();
    let selected_account_wait_delay_seconds = provider_accounts
        .iter()
        .find(|input| input.account_id() == excluded_account_id)
        .and_then(|account| {
            exhausted_account_short_quota_wait_delay_seconds(
                account,
                now_unix_seconds,
                short_quota_wait_jitter_seconds,
            )
        });
    let account_inputs = provider_accounts
        .iter()
        .filter(|input| input.account_id() != excluded_account_id)
        .map(|account| (*account).clone())
        .collect::<Vec<_>>();
    let account_inputs = match runtime_exhaustions {
        Some(runtime_exhaustions) => projected_accounts_excluding_runtime_exhaustions(
            account_inputs,
            runtime_exhaustions,
            route_band.as_str(),
            now_unix_seconds,
        )?,
        None => account_inputs,
    };
    let short_quota_wait_delay_seconds = short_quota_wait_delay_seconds_with_jitter(
        &account_inputs,
        now_unix_seconds,
        short_quota_wait_jitter_seconds,
    );
    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        route_band,
        now_unix_seconds,
        route_profile,
        account_inputs,
    ));
    if post_exhaustion_assessment_has_safe_known_fresh_alternative(&assessment)? {
        return Ok(PostExhaustionRouteBandOutcome::SelectableAlternative);
    }

    let retry_after_seconds = match (
        selected_account_wait_delay_seconds,
        short_quota_wait_delay_seconds,
    ) {
        (Some(selected), Some(alternative)) => Some(selected.min(alternative)),
        (Some(selected), None) => Some(selected),
        (None, alternative) => alternative,
    };
    if let Some(retry_after_seconds) = retry_after_seconds {
        if let Some(runtime_exhaustions) = runtime_exhaustions {
            let mut runtime_exhaustions =
                runtime_exhaustions
                    .lock()
                    .map_err(|_error| StateStoreError::Sqlite {
                        message: "runtime quota exhaustion state unavailable".to_owned(),
                    })?;
            if let Some(exhaustions) = runtime_exhaustions.get_mut(route_band.as_str())
                && let Some(exhaustion) = exhaustions
                    .iter_mut()
                    .find(|exhaustion| &exhaustion.account_id == excluded_account_id)
            {
                exhaustion.expires_unix_seconds = now_unix_seconds.saturating_add(
                    retry_after_seconds.saturating_sub(short_quota_wait_jitter_seconds),
                );
            }
        }
        return Ok(PostExhaustionRouteBandOutcome::ShortQuotaWait {
            retry_after_seconds,
        });
    }

    Ok(PostExhaustionRouteBandOutcome::NoSelectableAlternative)
}

pub(super) fn post_exhaustion_assessment_has_safe_known_fresh_alternative(
    assessment: &BurnDownRouteBandAssessmentResult,
) -> Result<bool, StateStoreError> {
    let selected_pool = assessment.selected_pool();
    if selected_pool == SelectedPool::None {
        if assessment_has_unavailable_quota_authority(assessment) {
            return Err(StateStoreError::Sqlite {
                message: "post-exhaustion alternative quota evidence unavailable".to_owned(),
            });
        }
        return Ok(false);
    }
    if selected_pool == SelectedPool::Unknown {
        return Err(StateStoreError::Sqlite {
            message: "post-exhaustion alternative quota evidence unavailable".to_owned(),
        });
    }

    let candidates = assessment.weighted_candidates();
    if candidates.is_empty() {
        return Ok(false);
    }

    let all_candidates_are_known_fresh = candidates.iter().all(|(account_id, _weight)| {
        assessment.accounts().iter().any(|account| {
            account.account_id() == account_id
                && account.freshness() == QuotaEvidenceFreshness::Fresh
                && matches!(
                    (
                        selected_pool,
                        account.availability(),
                        account.quota_evidence_reason()
                    ),
                    (SelectedPool::Usable, AccountAvailability::Usable, _)
                        | (SelectedPool::Reserve, AccountAvailability::Reserve, _)
                        | (
                            SelectedPool::LastResort,
                            AccountAvailability::Blocked,
                            codex_router_selection::burn_down::QuotaEvidenceReason::ShortWindowGuard
                        )
                )
        })
    });

    if all_candidates_are_known_fresh {
        Ok(true)
    } else {
        Err(StateStoreError::Sqlite {
            message: "post-exhaustion alternative quota evidence unavailable".to_owned(),
        })
    }
}
