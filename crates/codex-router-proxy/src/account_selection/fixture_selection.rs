use super::*;

/// Test fixture selector that hydrates account state from synchronous repositories.
#[cfg(test)]
pub struct RepositoryBackedAccountSelector<'a, R>
where
    R: AffinityRepository + SelectorQuotaRepository,
{
    state_repository: &'a R,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    minimum_account_hold_cooldown_seconds: u64,
    clock: UnixClock,
}

#[cfg(test)]
impl<'a, R> RepositoryBackedAccountSelector<'a, R>
where
    R: AffinityRepository + SelectorQuotaRepository,
{
    /// Creates a repository-backed selector.
    #[must_use]
    pub fn new(state_repository: &'a R) -> Self {
        Self {
            state_repository,
            weighted_selectors: Arc::new(Mutex::new(HashMap::new())),
            account_holds: Arc::new(Mutex::new(HashMap::new())),
            minimum_account_hold_cooldown_seconds: DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            clock: Arc::new(current_unix_seconds),
        }
    }

    /// Creates a repository-backed selector with process-lifetime weighted state.
    #[must_use]
    pub fn new_with_weighted_selector(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
    ) -> Self {
        Self {
            state_repository,
            weighted_selectors,
            account_holds,
            minimum_account_hold_cooldown_seconds: DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            clock: Arc::new(current_unix_seconds),
        }
    }

    /// Creates a repository-backed selector with process-lifetime runtime state.
    #[must_use]
    pub fn new_with_runtime(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
        minimum_account_hold_cooldown_seconds: u64,
        clock: UnixClock,
    ) -> Self {
        Self {
            state_repository,
            weighted_selectors,
            account_holds,
            minimum_account_hold_cooldown_seconds,
            clock,
        }
    }
}

#[cfg(test)]
impl<R> AccountDecisionSelector for RepositoryBackedAccountSelector<'_, R>
where
    R: AffinityRepository + SelectorQuotaRepository,
{
    fn select_upstream_account(
        &self,
        request: &HttpProxyRequest,
        _token_generation: TokenGeneration,
        affinity_secret: Option<&RouterAffinityHashSecret>,
    ) -> Result<SelectedAccountDecision, HttpProxyError> {
        let route_kind = route_kind_for_request(request)?;
        let route_band = route_kind.route_band();
        let route_profile = route_profile_for_kind(route_kind);
        let now_unix_seconds = (self.clock)();
        let selector_inputs = self
            .state_repository
            .selector_inputs_for_route_band(route_band.as_str(), now_unix_seconds)
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::StateUnavailable,
            })?;
        let selector_inputs = selector_inputs_excluding_attempted(selector_inputs, request);
        let selector_accounts = selector_inputs
            .iter()
            .map(account_input_from_selector_input)
            .collect::<Vec<_>>();
        let selector_accounts =
            filter_selector_accounts_for_provider(selector_accounts, route_profile.provider);
        let assessment_input = BurnDownRouteBandAssessmentInput::new(
            route_band,
            now_unix_seconds,
            route_profile.clone(),
            selector_accounts,
        );
        let assessment = assess_route_band(assessment_input);
        if assessment.selected_pool() == SelectedPool::None {
            return Err(empty_assessment_selection_error(&assessment));
        }

        let mut weighted_selectors =
            self.weighted_selectors
                .lock()
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
                })?;
        let weighted_selector = weighted_selectors
            .entry(route_band.as_str().to_owned())
            .or_insert_with(WeightedDeficitSelector::default);
        let mut account_holds =
            self.account_holds
                .lock()
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
                })?;
        if route_kind.previous_response_affinity_capable()
            && let Some(previous_response_id) = previous_response_id(request)?
        {
            let affinity_secret = affinity_secret.ok_or(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            })?;
            let affinity_key_hash =
                hash_previous_response_id(affinity_secret, &previous_response_id).map_err(
                    |_error| HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
                    },
                )?;
            let owner_lookup = self
                .state_repository
                .load_previous_response_owner(&affinity_key_hash, route_band.as_str())
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::StateUnavailable,
                })?;
            let owner = match owner_lookup {
                PreviousResponseAffinityOwnerLookup::Found(owner) => owner,
                PreviousResponseAffinityOwnerLookup::Missing => {
                    return Err(HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::AffinityOwnerMissing,
                    });
                }
                PreviousResponseAffinityOwnerLookup::Ambiguous => {
                    return Err(HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
                    });
                }
            };
            if request
                .excluded_accounts()
                .iter()
                .any(|account_id| account_id == owner.account_id())
            {
                return Err(HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
                });
            }
            return select_affinity_owner(
                route_band,
                route_profile.provider,
                owner.account_id(),
                &assessment,
                &mut account_holds,
                now_unix_seconds,
                "previous_response_affinity",
            );
        }

        select_from_burn_down_assessment(
            route_band.as_str(),
            route_profile.provider,
            &assessment,
            weighted_selector,
            &mut account_holds,
            self.minimum_account_hold_cooldown_seconds,
            now_unix_seconds,
        )
    }
}

#[cfg(test)]
fn selector_inputs_excluding_attempted(
    selector_inputs: Vec<SelectorQuotaInput>,
    request: &HttpProxyRequest,
) -> Vec<SelectorQuotaInput> {
    if request.excluded_accounts().is_empty() {
        return selector_inputs;
    }

    selector_inputs
        .into_iter()
        .filter(|input| {
            !request
                .excluded_accounts()
                .iter()
                .any(|excluded_account_id| excluded_account_id == input.account_id())
        })
        .collect()
}

#[cfg(test)]
fn account_input_from_selector_input(input: &SelectorQuotaInput) -> BurnDownAccountInput {
    let windows = input
        .windows()
        .iter()
        .map(quota_window_fact_from_selector_window)
        .collect::<Vec<_>>();

    BurnDownAccountInput::new(
        input.account_id().clone(),
        input.account_label(),
        input.provider(),
        windows,
    )
    .with_account_enabled(input.account_status() == AccountStatus::Enabled)
    .with_active_credential(input.active_credential_generation().is_some())
}

#[cfg(test)]
fn quota_window_fact_from_selector_window(
    window: &PersistedSelectorQuotaWindow,
) -> QuotaWindowFact {
    let mut fact = QuotaWindowFact::new(
        window.limit_window_seconds(),
        quota_window_status_from_selector_status(window.status()),
    )
    .with_remaining_headroom(window.remaining_headroom())
    .with_observed_unix_seconds(window.observed_unix_seconds())
    .with_effective(window.effective());
    if let Some(reset_unix_seconds) = window.reset_unix_seconds() {
        fact = fact.with_reset_unix_seconds(reset_unix_seconds);
    }

    fact
}

#[cfg(test)]
const fn quota_window_status_from_selector_status(
    status: SelectorQuotaWindowStatus,
) -> QuotaWindowStatus {
    match status {
        SelectorQuotaWindowStatus::Eligible => QuotaWindowStatus::Eligible,
        SelectorQuotaWindowStatus::Stale => QuotaWindowStatus::Stale,
        SelectorQuotaWindowStatus::Unknown => QuotaWindowStatus::Unknown,
        SelectorQuotaWindowStatus::Ineligible => QuotaWindowStatus::Ineligible,
    }
}
