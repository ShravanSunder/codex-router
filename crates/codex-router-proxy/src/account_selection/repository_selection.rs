use super::*;

const MAX_CLAUDE_ADMISSION_PIN_RELEASE_ATTEMPTS: usize = 2;

impl<'a, R> AsyncRepositoryBackedAccountSelector<'a, R>
where
    R: AsyncAffinityRepository
        + AsyncSessionAccountAffinityRepository
        + AsyncSelectionProjectionRepository
        + Sync,
{
    /// Creates an async repository-backed selector.
    #[must_use]
    pub fn new(state_repository: &'a R) -> Self {
        Self {
            state_repository,
            claude_affinity_writer: state_repository,
            weighted_selectors: Arc::new(Mutex::new(HashMap::new())),
            account_holds: Arc::new(Mutex::new(HashMap::new())),
            active_reservations: Arc::new(Mutex::new(HashMap::new())),
            runtime_exhaustions: Arc::new(Mutex::new(HashMap::new())),
            route_band_queue_health: Arc::new(Mutex::new(HashMap::new())),
            active_client_leases: None,
            session_affinity_writer: None,
            session_affinity_cache: SessionAccountAffinityCache::shared(
                DEFAULT_SESSION_PIN_IDLE_TTL,
            ),
            selection_reservation_lock: Arc::new(AsyncMutex::new(())),
            minimum_account_hold_cooldown_seconds: DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            clock: Arc::new(current_unix_seconds),
            claude_five_hour_reserve_percent: DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT,
        }
    }

    /// Creates an async repository-backed selector with process-lifetime weighted state.
    #[must_use]
    pub fn new_with_weighted_selector(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
    ) -> Self {
        Self {
            state_repository,
            claude_affinity_writer: state_repository,
            weighted_selectors,
            account_holds,
            active_reservations: Arc::new(Mutex::new(HashMap::new())),
            runtime_exhaustions: Arc::new(Mutex::new(HashMap::new())),
            route_band_queue_health: Arc::new(Mutex::new(HashMap::new())),
            active_client_leases: None,
            session_affinity_writer: None,
            session_affinity_cache: SessionAccountAffinityCache::shared(
                DEFAULT_SESSION_PIN_IDLE_TTL,
            ),
            selection_reservation_lock: Arc::new(AsyncMutex::new(())),
            minimum_account_hold_cooldown_seconds: DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            clock: Arc::new(current_unix_seconds),
            claude_five_hour_reserve_percent: DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT,
        }
    }

    /// Creates an async repository-backed selector with process-lifetime runtime state.
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
            claude_affinity_writer: state_repository,
            weighted_selectors,
            account_holds,
            active_reservations: Arc::new(Mutex::new(HashMap::new())),
            runtime_exhaustions: Arc::new(Mutex::new(HashMap::new())),
            route_band_queue_health: Arc::new(Mutex::new(HashMap::new())),
            active_client_leases: None,
            session_affinity_writer: None,
            session_affinity_cache: SessionAccountAffinityCache::shared(
                DEFAULT_SESSION_PIN_IDLE_TTL,
            ),
            selection_reservation_lock: Arc::new(AsyncMutex::new(())),
            minimum_account_hold_cooldown_seconds,
            clock,
            claude_five_hour_reserve_percent: DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT,
        }
    }

    /// Creates an async selector with explicit active reservation state.
    #[must_use]
    pub fn new_with_runtime_and_reservations(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
        active_reservations: RouteBandReservationBooks,
        minimum_account_hold_cooldown_seconds: u64,
        clock: UnixClock,
    ) -> Self {
        Self {
            state_repository,
            claude_affinity_writer: state_repository,
            weighted_selectors,
            account_holds,
            active_reservations,
            runtime_exhaustions: Arc::new(Mutex::new(HashMap::new())),
            route_band_queue_health: Arc::new(Mutex::new(HashMap::new())),
            active_client_leases: None,
            session_affinity_writer: None,
            session_affinity_cache: SessionAccountAffinityCache::shared(
                DEFAULT_SESSION_PIN_IDLE_TTL,
            ),
            selection_reservation_lock: Arc::new(AsyncMutex::new(())),
            minimum_account_hold_cooldown_seconds,
            clock,
            claude_five_hour_reserve_percent: DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT,
        }
    }

    /// Creates an async selector with explicit process-local runtime state.
    #[must_use]
    pub fn new_with_runtime_state(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
        active_reservations: RouteBandReservationBooks,
        runtime_exhaustions: RouteBandRuntimeExhaustions,
        minimum_account_hold_cooldown_seconds: u64,
        clock: UnixClock,
    ) -> Self {
        Self::new_with_runtime_dependencies(
            state_repository,
            AsyncAccountSelectorRuntimeState::new(
                weighted_selectors,
                account_holds,
                active_reservations,
                runtime_exhaustions,
                Arc::new(Mutex::new(HashMap::new())),
            ),
            minimum_account_hold_cooldown_seconds,
            clock,
        )
    }

    /// Creates an async selector with explicit process-local runtime dependencies.
    #[must_use]
    pub fn new_with_runtime_dependencies(
        state_repository: &'a R,
        runtime_state: AsyncAccountSelectorRuntimeState,
        minimum_account_hold_cooldown_seconds: u64,
        clock: UnixClock,
    ) -> Self {
        Self {
            state_repository,
            claude_affinity_writer: state_repository,
            weighted_selectors: runtime_state.weighted_selectors,
            account_holds: runtime_state.account_holds,
            active_reservations: runtime_state.active_reservations,
            runtime_exhaustions: runtime_state.runtime_exhaustions,
            route_band_queue_health: runtime_state.route_band_queue_health,
            active_client_leases: None,
            session_affinity_writer: None,
            session_affinity_cache: runtime_state.session_affinity_cache,
            selection_reservation_lock: runtime_state.selection_reservation_lock,
            minimum_account_hold_cooldown_seconds,
            clock,
            claude_five_hour_reserve_percent: DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT,
        }
    }

    /// Adds process-external active client lease reporting.
    #[must_use]
    pub fn with_active_client_lease_reporter(
        mut self,
        active_client_leases: Arc<dyn ActiveClientLeaseReporter>,
    ) -> Self {
        self.active_client_leases = Some(active_client_leases);
        self
    }

    /// Persists Codex session account affinity through the existing DB writer.
    #[must_use]
    pub fn with_session_affinity_writer(mut self, db_write_actor: DbWriteActor) -> Self {
        self.session_affinity_writer = Some(db_write_actor);
        self
    }

    /// Routes Claude pin CAS through a writable repository while reads keep their owner.
    #[must_use]
    pub(crate) fn with_claude_affinity_writer(
        mut self,
        writer: &'a (dyn AsyncSessionAccountAffinityRepository + Sync),
    ) -> Self {
        self.claude_affinity_writer = writer;
        self
    }

    /// Sets the Claude five-hour Reserve threshold supplied by runtime configuration.
    #[must_use]
    pub const fn with_claude_five_hour_reserve_percent(
        mut self,
        percent: ClaudeFiveHourReservePercent,
    ) -> Self {
        self.claude_five_hour_reserve_percent = percent;
        self
    }
}

impl<R> AsyncAccountDecisionSelector for AsyncRepositoryBackedAccountSelector<'_, R>
where
    R: AsyncAffinityRepository
        + AsyncSessionAccountAffinityRepository
        + AsyncSelectionProjectionRepository
        + Sync,
{
    fn select_upstream_account<'a>(
        &'a self,
        request: &'a HttpProxyRequest,
        _token_generation: TokenGeneration,
        affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            let route_kind = route_kind_for_request(request)?;
            let route_band = route_kind.route_band();
            let route_profile = route_profile_for_kind(route_kind)
                .with_claude_five_hour_reserve_percent(self.claude_five_hour_reserve_percent);
            let _selection_reservation_guard = self.selection_reservation_lock.lock().await;
            let now_unix_seconds = (self.clock)();
            route_band_queue_health_allows_selection(&self.route_band_queue_health, route_band)
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::StateUnavailable,
                })?;
            let active_reservation_book = active_reservation_book_for_route_band(
                &self.active_reservations,
                route_band.as_str(),
                now_unix_seconds,
            )?;
            let active_session_overrides = active_reservation_book
                .as_ref()
                .map(active_session_counts_by_account);
            let projection = project_route_band_selection_inputs_with_active_counts_read_only(
                self.state_repository,
                route_band.as_str(),
                now_unix_seconds,
                ACTIVE_RESERVATION_MAX_AGE_SECONDS,
                active_session_overrides.as_ref(),
            )
            .await
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::StateUnavailable,
            })?;
            let selector_accounts =
                projected_accounts_excluding_attempted(projection.accounts(), request);
            let selector_accounts = projected_accounts_excluding_runtime_exhaustions(
                selector_accounts,
                &self.runtime_exhaustions,
                route_band.as_str(),
                now_unix_seconds,
            )
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
            })?;
            let selector_accounts =
                filter_selector_accounts_for_provider(selector_accounts, route_profile.provider);
            let short_quota_wait_delay_seconds =
                short_quota_wait_delay_seconds(&selector_accounts, now_unix_seconds);
            let assessment_input = BurnDownRouteBandAssessmentInput::new(
                route_band,
                now_unix_seconds,
                route_profile.clone(),
                selector_accounts,
            );
            let assessment = assess_route_band(assessment_input);
            let affinity_owner_account_id = if route_kind.previous_response_affinity_capable() {
                match previous_response_id(request)? {
                    Some(previous_response_id) => {
                        let affinity_secret = affinity_secret.ok_or(HttpProxyError::Selection {
                            reason: QuotaAwareAccountSelectorError::SecretUnavailable,
                        })?;
                        let affinity_key_hash =
                            hash_previous_response_id(affinity_secret, &previous_response_id)
                                .map_err(|_error| HttpProxyError::Selection {
                                    reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
                                })?;
                        let owner_lookup = AsyncAffinityRepository::load_previous_response_owner(
                            self.state_repository,
                            &affinity_key_hash,
                            route_band.as_str(),
                        )
                        .await
                        .map_err(|_error| HttpProxyError::Selection {
                            reason: QuotaAwareAccountSelectorError::StateUnavailable,
                        })?;
                        Some(account_id_from_affinity_owner_lookup(owner_lookup)?)
                    }
                    None => None,
                }
            } else {
                None
            };
            let session_id = session_id_for_route(request, route_kind);
            let mut pin_observation =
                match session_id.filter(|_| route_profile.provider == Provider::Claude) {
                    Some(session_id) => Some(
                        observe_claude_session_account_affinity(
                            &self.session_affinity_cache,
                            session_id,
                            self.state_repository,
                            (self.clock)(),
                        )
                        .await
                        .map_err(|_error| HttpProxyError::Selection {
                            reason: QuotaAwareAccountSelectorError::StateUnavailable,
                        })?,
                    ),
                    None => None,
                };
            let reserve_release_required = pin_observation
                .as_ref()
                .and_then(PinObservation::active_account)
                .is_some_and(|account_id| {
                    assessment_account_must_yield(&assessment, account_id, &route_profile)
                });
            let release_attempt_limit = if reserve_release_required {
                MAX_CLAUDE_ADMISSION_PIN_RELEASE_ATTEMPTS
            } else {
                1
            };
            let mut select_afresh_after_pin_release_contention = false;
            for release_attempt in 0..release_attempt_limit {
                let Some(session_id) = session_id else { break };
                let Some(observation) = pin_observation.as_ref() else {
                    break;
                };
                if !observation.active_account().is_some_and(|account_id| {
                    !assessment_account_is_available(&assessment, account_id)
                        || assessment_account_must_yield(&assessment, account_id, &route_profile)
                }) {
                    break;
                }
                let release = release_claude_session_account_affinity(
                    &self.session_affinity_cache,
                    session_id,
                    observation,
                    self.state_repository,
                    self.claude_affinity_writer,
                    (self.clock)(),
                )
                .await
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::StateUnavailable,
                })?;
                select_afresh_after_pin_release_contention = reserve_release_required
                    && release_attempt + 1 == release_attempt_limit
                    && !release.released;
                pin_observation = Some(release.observation);
                if release.released {
                    break;
                }
            }
            if assessment.selected_pool() == SelectedPool::None {
                if affinity_owner_account_id.as_ref().is_some_and(|owner_id| {
                    assessment.accounts().iter().any(|account| {
                        account.account_id() == owner_id
                            && account.routing_exclusion() == RoutingExclusion::WeeklyQuotaFloor
                    })
                }) {
                    return Err(HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
                    });
                }
                if let Some(retry_after_seconds) = short_quota_wait_delay_seconds {
                    return Err(HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::ShortQuotaExhausted {
                            retry_after_seconds,
                        },
                    });
                }
                return Err(empty_assessment_selection_error(&assessment));
            }
            let session_affinity = match session_affinity_lookup_session_id(
                session_id.filter(|_| route_profile.provider != Provider::Claude),
                affinity_owner_account_id.is_some(),
            ) {
                Some(session_id) => {
                    if let Some(cached) = lookup_session_account_affinity(
                        &self.session_affinity_cache,
                        route_profile.provider,
                        session_id,
                        route_band,
                        self.session_affinity_writer.as_ref(),
                        now_unix_seconds,
                    )
                    .map_err(|_error| HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::StateUnavailable,
                    })? {
                        Some(cached)
                    } else {
                        let persisted =
                            AsyncSessionAccountAffinityRepository::load_session_account_affinity(
                                self.state_repository,
                                route_profile.provider,
                                session_id,
                            )
                            .await
                            .map_err(|_error| {
                                HttpProxyError::Selection {
                                    reason: QuotaAwareAccountSelectorError::StateUnavailable,
                                }
                            })?;
                        reconcile_persisted_session_account_affinity(
                            &self.session_affinity_cache,
                            route_profile.provider,
                            session_id,
                            persisted.as_ref(),
                            route_band,
                            self.session_affinity_writer.as_ref(),
                            (self.clock)(),
                        )
                        .map_err(|_error| HttpProxyError::Selection {
                            reason: QuotaAwareAccountSelectorError::StateUnavailable,
                        })?
                    }
                }
                None => None,
            };

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

            if let Some(owner_account_id) = affinity_owner_account_id {
                if request
                    .excluded_accounts()
                    .iter()
                    .any(|account_id| account_id == &owner_account_id)
                {
                    return Err(HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
                    });
                }
                let selected = select_affinity_owner(
                    route_band,
                    route_profile.provider,
                    &owner_account_id,
                    &assessment,
                    &mut account_holds,
                    now_unix_seconds,
                    "previous_response_affinity",
                )?;
                let selected = reserve_selected_account(
                    selected,
                    &self.active_reservations,
                    self.active_client_leases.as_ref(),
                    route_band.as_str(),
                    transport_label_for_request(request),
                    now_unix_seconds,
                )?;
                return publish_selected_session_affinity(
                    selected,
                    route_profile.provider,
                    &self.session_affinity_cache,
                    self.session_affinity_writer.as_ref(),
                    session_id,
                    route_band,
                    now_unix_seconds,
                );
            }

            let pinned_account_id = pin_observation
                .as_ref()
                .and_then(PinObservation::active_account)
                .or_else(|| {
                    session_affinity
                        .as_ref()
                        .map(|affinity| affinity.account_id())
                });
            if let Some(pinned_account_id) = pinned_account_id
                && !select_afresh_after_pin_release_contention
                && assessment_account_is_available(&assessment, pinned_account_id)
                && !assessment_account_must_yield(&assessment, pinned_account_id, &route_profile)
            {
                let selected = select_affinity_owner(
                    route_band,
                    route_profile.provider,
                    pinned_account_id,
                    &assessment,
                    &mut account_holds,
                    now_unix_seconds,
                    "prompt_cache_account_affinity",
                )?;
                let selected = reserve_selected_account(
                    selected,
                    &self.active_reservations,
                    self.active_client_leases.as_ref(),
                    route_band.as_str(),
                    transport_label_for_request(request),
                    now_unix_seconds,
                )?;
                return publish_selected_session_affinity(
                    selected.with_pin_observation(pin_observation),
                    route_profile.provider,
                    &self.session_affinity_cache,
                    self.session_affinity_writer.as_ref(),
                    session_id,
                    route_band,
                    now_unix_seconds,
                );
            }

            let selected = select_from_burn_down_assessment(
                route_band.as_str(),
                route_profile.provider,
                &assessment,
                weighted_selector,
                &mut account_holds,
                self.minimum_account_hold_cooldown_seconds,
                now_unix_seconds,
            )?;
            let selected = reserve_selected_account(
                selected,
                &self.active_reservations,
                self.active_client_leases.as_ref(),
                route_band.as_str(),
                transport_label_for_request(request),
                now_unix_seconds,
            )?;
            publish_selected_session_affinity(
                selected.with_pin_observation(pin_observation),
                route_profile.provider,
                &self.session_affinity_cache,
                self.session_affinity_writer.as_ref(),
                session_id,
                route_band,
                now_unix_seconds,
            )
        })
    }
}
