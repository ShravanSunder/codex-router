use super::affinity_admission::assessment_account_is_credit_backed;
use super::*;

impl QuotaAwareAccountState {
    /// Creates account state for selector input.
    #[must_use]
    pub const fn new(
        account_id: AccountId,
        remaining_headroom: u32,
        freshness: SnapshotFreshness,
    ) -> Self {
        Self {
            account_id,
            remaining_headroom,
            freshness,
        }
    }
}

impl QuotaAwareAccountSelector {
    /// Creates a quota-aware selector from account snapshots.
    #[must_use]
    pub fn new(accounts: Vec<QuotaAwareAccountState>) -> Self {
        Self {
            accounts,
            weighted_selector: Mutex::new(WeightedDeficitSelector::default()),
        }
    }
}

impl AccountDecisionSelector for QuotaAwareAccountSelector {
    fn select_upstream_account(
        &self,
        request: &HttpProxyRequest,
        _token_generation: TokenGeneration,
        _affinity_secret: Option<&RouterAffinityHashSecret>,
    ) -> Result<SelectedAccountDecision, HttpProxyError> {
        let selectable_accounts = quota_states_excluding_attempted(&self.accounts, request);
        select_from_account_states(&selectable_accounts, &self.weighted_selector)
    }
}

fn select_from_account_states(
    accounts: &[QuotaAwareAccountState],
    weighted_selector: &Mutex<WeightedDeficitSelector>,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    let mut weighted_selector =
        weighted_selector
            .lock()
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
            })?;
    select_from_account_states_with_selector(accounts, &mut weighted_selector, current_unix_seconds)
}

fn quota_states_excluding_attempted(
    accounts: &[QuotaAwareAccountState],
    request: &HttpProxyRequest,
) -> Vec<QuotaAwareAccountState> {
    if request.excluded_accounts().is_empty() {
        return accounts.to_vec();
    }

    accounts
        .iter()
        .filter(|account| {
            !request
                .excluded_accounts()
                .iter()
                .any(|excluded_account_id| excluded_account_id == &account.account_id)
        })
        .cloned()
        .collect()
}

pub(super) fn projected_accounts_excluding_attempted(
    accounts: &[BurnDownAccountInput],
    request: &HttpProxyRequest,
) -> Vec<BurnDownAccountInput> {
    if request.excluded_accounts().is_empty() {
        return accounts.to_vec();
    }

    accounts
        .iter()
        .filter(|account| {
            !request
                .excluded_accounts()
                .iter()
                .any(|excluded_account_id| excluded_account_id == account.account_id())
        })
        .cloned()
        .collect()
}

pub(super) fn select_from_account_states_with_selector(
    accounts: &[QuotaAwareAccountState],
    weighted_selector: &mut WeightedDeficitSelector,
    mut clock: impl FnMut() -> u64,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    let now_unix_seconds = clock();
    let account_inputs = accounts
        .iter()
        .map(|account| account_input_from_quota_state(account, now_unix_seconds))
        .collect::<Vec<_>>();
    let assessment_input = BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        RESPONSES_HTTP.clone(),
        account_inputs,
    );
    let assessment = assess_route_band(assessment_input);
    select_from_burn_down_assessment_without_hold(&assessment, weighted_selector)
}

pub(super) fn select_from_burn_down_assessment_without_hold(
    assessment: &BurnDownRouteBandAssessmentResult,
    _weighted_selector: &mut WeightedDeficitSelector,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    if assessment.selected_pool() == SelectedPool::None {
        return Err(empty_assessment_selection_error(assessment));
    }
    let selected_account_id = strict_preferred_account_id(assessment)?;
    let selected_assessment = assessment
        .accounts()
        .iter()
        .find(|account| account.account_id() == &selected_account_id)
        .ok_or(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        })?;

    Ok(SelectedAccountDecision::new(
        selected_account_id,
        selection_reason_for_assessment(selected_assessment),
    )
    .with_credit_backed_at_selection(
        selected_assessment.quota_evidence_reason() == QuotaEvidenceReason::CreditBacked,
    ))
}

pub(super) fn select_from_burn_down_assessment(
    route_band: &str,
    provider: Provider,
    assessment: &BurnDownRouteBandAssessmentResult,
    _weighted_selector: &mut WeightedDeficitSelector,
    account_holds: &mut HashMap<ProviderRouteBand, AccountHold>,
    minimum_account_hold_cooldown_seconds: u64,
    now_unix_seconds: u64,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    let selection_scope = ProviderRouteBand::new(provider, assessment.route_band());
    let weighted_candidates = assessment.weighted_candidates();
    if weighted_candidates.is_empty() {
        account_holds.remove(&selection_scope);
        return Err(empty_assessment_selection_error(assessment));
    }

    if let Some(held_account_id) = reusable_held_account_id(
        selection_scope,
        account_holds,
        weighted_candidates,
        minimum_account_hold_cooldown_seconds,
        now_unix_seconds,
    ) && Some(&held_account_id) == assessment.preferred_next()
    {
        return Ok(
            SelectedAccountDecision::new(held_account_id.clone(), "account_hold_cooldown")
                .with_credit_backed_at_selection(assessment_account_is_credit_backed(
                    assessment,
                    &held_account_id,
                )),
        );
    }

    let selected_account_id = strict_preferred_account_id(assessment)?;
    let selected_assessment = assessment
        .accounts()
        .iter()
        .find(|account| account.account_id() == &selected_account_id)
        .ok_or(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        })?;
    account_holds.insert(
        selection_scope,
        AccountHold::new(selected_account_id.clone(), now_unix_seconds),
    );

    let selected_reason = selection_reason_for_assessment(selected_assessment);
    tracing::info!(target: "codex_router_proxy::account_selection",
        route_band,
        account.hash = telemetry_hash(selected_account_id.as_str()),
        selection.reason = selected_reason.as_str(),
        "codex_router.account_selected"
    );

    Ok(
        SelectedAccountDecision::new(selected_account_id, selected_reason)
            .with_credit_backed_at_selection(
                selected_assessment.quota_evidence_reason() == QuotaEvidenceReason::CreditBacked,
            ),
    )
}

pub(super) fn empty_assessment_selection_error(
    assessment: &BurnDownRouteBandAssessmentResult,
) -> HttpProxyError {
    let route_band = assessment.route_band().as_str();
    if assessment_has_unavailable_quota_authority(assessment) {
        crate::telemetry::record_account_rejected(route_band, "quota_authority_unavailable");
        tracing::warn!(target: "codex_router_proxy::account_selection",
            route_band,
            reason = "quota_authority_unavailable",
            "codex_router.account_selection_rejected"
        );
        return HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::StateUnavailable,
        };
    }

    account_selection_rejected_no_eligible(route_band)
}

pub(super) fn assessment_has_unavailable_quota_authority(
    assessment: &BurnDownRouteBandAssessmentResult,
) -> bool {
    assessment.accounts().iter().any(|account| {
        matches!(
            account.freshness(),
            QuotaEvidenceFreshness::Stale | QuotaEvidenceFreshness::Unknown
        ) && matches!(
            account.routing_exclusion(),
            RoutingExclusion::None | RoutingExclusion::WeeklyQuotaFloor
        )
    })
}

fn strict_preferred_account_id(
    assessment: &BurnDownRouteBandAssessmentResult,
) -> Result<AccountId, HttpProxyError> {
    let typed_choice =
        SelectionOutcome::chosen_from_assessment(assessment).and_then(|outcome| match outcome {
            SelectionOutcome::Chosen { account, .. } => Some(account),
            SelectionOutcome::Unavailable(_) => None,
        });
    typed_choice
        .or_else(|| assessment.preferred_next().cloned())
        .or_else(|| {
            assessment
                .weighted_candidates()
                .first()
                .map(|(account_id, _weight)| account_id.clone())
        })
        .ok_or(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        })
}

fn account_selection_rejected_no_eligible(route_band: &str) -> HttpProxyError {
    crate::telemetry::record_account_rejected(route_band, "no_eligible_accounts");
    tracing::warn!(target: "codex_router_proxy::account_selection",
        route_band,
        reason = "no_eligible_accounts",
        "codex_router.account_selection_rejected"
    );
    HttpProxyError::Selection {
        reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
    }
}

fn reusable_held_account_id(
    selection_scope: ProviderRouteBand,
    account_holds: &mut HashMap<ProviderRouteBand, AccountHold>,
    weighted_candidates: &[(AccountId, u32)],
    minimum_account_hold_cooldown_seconds: u64,
    now_unix_seconds: u64,
) -> Option<AccountId> {
    let hold = account_holds.get(&selection_scope)?;
    let hold_age_seconds = now_unix_seconds.saturating_sub(hold.selected_unix_seconds);
    let reusable = hold_age_seconds < minimum_account_hold_cooldown_seconds
        && weighted_candidates
            .iter()
            .any(|(account_id, _weight)| account_id == &hold.account_id);
    if reusable {
        Some(hold.account_id.clone())
    } else {
        account_holds.remove(&selection_scope);
        None
    }
}

pub(super) fn filter_selector_accounts_for_provider(
    accounts: Vec<BurnDownAccountInput>,
    provider: Provider,
) -> Vec<BurnDownAccountInput> {
    accounts
        .into_iter()
        .filter(|account| account.provider() == provider)
        .collect()
}

fn account_input_from_quota_state(
    account: &QuotaAwareAccountState,
    now_unix_seconds: u64,
) -> BurnDownAccountInput {
    let status = quota_window_status_from_freshness(account.freshness);
    let short_window = QuotaWindowFact::new(V1_SHORT_WINDOW_SECONDS, status)
        .with_remaining_headroom(account.remaining_headroom)
        .with_reset_unix_seconds(now_unix_seconds)
        .with_observed_unix_seconds(now_unix_seconds)
        .with_effective(true);
    let weekly_window = QuotaWindowFact::new(V1_WEEKLY_WINDOW_SECONDS, status)
        .with_remaining_headroom(account.remaining_headroom)
        .with_reset_unix_seconds(now_unix_seconds)
        .with_observed_unix_seconds(now_unix_seconds)
        .with_effective(false);
    BurnDownAccountInput::new(
        account.account_id.clone(),
        account.account_id.as_str(),
        Provider::Openai,
        vec![short_window, weekly_window],
    )
}

const fn quota_window_status_from_freshness(freshness: SnapshotFreshness) -> QuotaWindowStatus {
    match freshness {
        SnapshotFreshness::Fresh { .. } => QuotaWindowStatus::Eligible,
        SnapshotFreshness::StaleWithPenalty { .. } => QuotaWindowStatus::Stale,
        SnapshotFreshness::Unknown => QuotaWindowStatus::Unknown,
    }
}

fn selection_reason_for_assessment(assessment: &BurnDownAccountAssessment) -> String {
    assessment.routing_reason().as_str().to_owned()
}
