use super::*;

#[derive(Clone)]
pub(super) struct SlowSelectionProjectionRepository {
    account_ids: Vec<AccountId>,
    pub(super) provider: Provider,
    pub(super) short_headroom: HashMap<AccountId, u32>,
    pub(super) release_contention_pins:
        Arc<Mutex<VecDeque<codex_router_state::session_account_affinity::SessionAccountAffinity>>>,
    pub(super) release_compare_and_set_count: Arc<AtomicUsize>,
    in_flight_selector_reads: Arc<std::sync::atomic::AtomicUsize>,
    max_concurrent_selector_reads: Arc<std::sync::atomic::AtomicUsize>,
    affinity_read: Option<Arc<BlockingAffinityRead>>,
    pub(super) persisted_affinities:
        Arc<Mutex<Vec<codex_router_state::session_account_affinity::SessionAccountAffinity>>>,
}

struct BlockingAffinityRead {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl SlowSelectionProjectionRepository {
    pub(super) fn new(account_id: AccountId) -> Self {
        Self::new_with_accounts(vec![account_id])
    }

    pub(super) fn new_with_accounts(account_ids: Vec<AccountId>) -> Self {
        Self {
            account_ids,
            provider: Provider::Openai,
            short_headroom: HashMap::new(),
            release_contention_pins: Arc::new(Mutex::new(VecDeque::new())),
            release_compare_and_set_count: Arc::new(AtomicUsize::new(0)),
            in_flight_selector_reads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            max_concurrent_selector_reads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            affinity_read: None,
            persisted_affinities: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn new_with_blocking_affinity(
        account_ids: Vec<AccountId>,
        persisted: codex_router_state::session_account_affinity::SessionAccountAffinity,
    ) -> Self {
        let mut repository = Self::new_with_accounts(account_ids);
        repository.persisted_affinities = Arc::new(Mutex::new(vec![persisted]));
        repository.affinity_read = Some(Arc::new(BlockingAffinityRead {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }));
        repository
    }

    pub(super) async fn wait_for_affinity_read(&self) {
        let affinity_read = self
            .affinity_read
            .as_ref()
            .unwrap_or_else(|| panic!("blocking affinity read should be configured"));
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            affinity_read.entered.notified(),
        )
        .await
        .unwrap_or_else(|_elapsed| panic!("selector should await persisted affinity"));
    }

    pub(super) fn release_affinity_read(&self) {
        self.affinity_read
            .as_ref()
            .unwrap_or_else(|| panic!("blocking affinity read should be configured"))
            .release
            .notify_one();
    }

    pub(super) fn max_concurrent_selector_reads(&self) -> usize {
        self.max_concurrent_selector_reads
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl codex_router_state::selection_projection::AsyncSelectionProjectionRepository
    for SlowSelectionProjectionRepository
{
    fn list_account_routing_policies(
        &self,
    ) -> futures_util::future::BoxFuture<
        '_,
        Result<
            Vec<codex_router_state::account_routing_policy::AccountRoutingPolicy>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn selector_inputs_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        _now_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::quota_snapshot::SelectorQuotaInput>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async move {
            let in_flight = self
                .in_flight_selector_reads
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            self.max_concurrent_selector_reads
                .fetch_max(in_flight, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            self.in_flight_selector_reads
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self
                    .account_ids
                    .iter()
                    .map(|account_id| {
                        let input = codex_router_state::quota_snapshot::SelectorQuotaInput::new(
                            account_id.clone(),
                            account_id.as_str(),
                            self.provider,
                            codex_router_state::account::AccountStatus::Enabled,
                            Some(1),
                            route_band,
                            vec![
                        codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow::new(
                            account_id.clone(),
                            route_band,
                            codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                            codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Eligible,
                        )
                        .with_remaining_headroom(self.short_headroom.get(account_id).copied().unwrap_or(90))
                        .with_reset_unix_seconds(18_000)
                        .with_effective(true)
                        .with_observed_unix_seconds(900),
                        codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow::new(
                            account_id.clone(),
                            route_band,
                            codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                            codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Eligible,
                        )
                        .with_remaining_headroom(90)
                        .with_reset_unix_seconds(1_000 + 24 * 3_600)
                        .with_effective(true)
                        .with_observed_unix_seconds(900),
                        ],
                        );
                        if self.provider == Provider::Claude {
                            let observations = [
                                (codex_router_core::route_profile::WindowKind::FiveHour,
                                 self.short_headroom.get(account_id).copied().unwrap_or(90) * 100),
                                (codex_router_core::route_profile::WindowKind::Weekly, 9_000),
                            ].into_iter().map(|(window_kind, remaining)| {
                                codex_router_state::window_observation::WindowObservation::new(
                                    codex_router_state::window_observation::WindowObservationProps::new(
                                        account_id.clone(), window_kind, remaining, 900,
                                    ).with_reset_unix_seconds(18_000)
                                     .with_fresh_until_unix_seconds(2_000),
                                ).unwrap_or_else(|error| panic!("test Claude observation: {error}"))
                            }).collect();
                            input.with_window_state(observations, Vec::new())
                        } else {
                            input
                        }
                    })
                    .collect())
        })
    }

    fn active_client_counts_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::sqlite::ActiveClientCount>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn active_client_counts_for_route_band_read_only<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::sqlite::ActiveClientCount>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn quota_history_observations_for_window<'a>(
        &'a self,
        _account_id: &'a AccountId,
        _route_band: &'a str,
        _limit_window_seconds: u64,
        _observed_from_unix_seconds: u64,
        _observed_to_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::quota_snapshot::PersistedQuotaHistoryObservation>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn active_session_rollups_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::sqlite::ActiveSessionRollup>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn refresh_active_session_rollups_for_interval<'a>(
        &'a self,
        _route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
        _bucket_seconds: u64,
    ) -> futures_util::future::BoxFuture<'a, Result<(), codex_router_state::sqlite::StateStoreError>>
    {
        Box::pin(async { panic!("read-only shared-lock test must not refresh rollups") })
    }
}

impl codex_router_state::sqlite::AsyncAffinityRepository for SlowSelectionProjectionRepository {
    fn write_previous_response_owner<'a>(
        &'a self,
        _owner: &'a codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord,
    ) -> futures_util::future::BoxFuture<'a, Result<(), codex_router_state::sqlite::StateStoreError>>
    {
        Box::pin(async { Ok(()) })
    }

    fn load_previous_response_owner<'a>(
        &'a self,
        _affinity_key_hash: &'a codex_router_core::affinity::AffinityKeyHash,
        _route_band: &'a str,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async {
            Ok(codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup::Missing)
        })
    }
}

impl codex_router_state::sqlite::AsyncSessionAccountAffinityRepository
    for SlowSelectionProjectionRepository
{
    fn upsert_session_account_affinity<'a>(
        &'a self,
        affinity: &'a codex_router_state::session_account_affinity::SessionAccountAffinity,
    ) -> futures_util::future::BoxFuture<'a, Result<(), codex_router_state::sqlite::StateStoreError>>
    {
        Box::pin(async move {
            let mut persisted_affinities = self
                .persisted_affinities
                .lock()
                .expect("test affinity store lock should not be poisoned");
            if let Some(existing) = persisted_affinities.iter_mut().find(|existing| {
                existing.provider() == affinity.provider()
                    && existing.session_id() == affinity.session_id()
            }) {
                *existing = affinity.clone();
            } else {
                persisted_affinities.push(affinity.clone());
            }
            Ok(())
        })
    }

    fn compare_and_set_session_account_affinity<'a>(
        &'a self,
        observation: &'a codex_router_state::session_account_affinity::PinObservation,
        affinity: &'a codex_router_state::session_account_affinity::SessionAccountAffinity,
        pin_ttl_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<bool, codex_router_state::sqlite::StateStoreError>,
    > {
        Box::pin(async move {
            let expected_version = observation.version();
            let next_version = expected_version.checked_add(1);
            let desired_version = affinity.pin_version();
            let valid_transition = match (observation.active_account(), affinity.account_id()) {
                (None, Some(_)) => next_version == Some(desired_version),
                (Some(observed), Some(replacement)) => {
                    observed == replacement && desired_version == expected_version
                }
                (Some(_), None) => next_version == Some(desired_version),
                (None, None) => false,
            };
            if !valid_transition {
                return Ok(false);
            }

            if affinity.account_id().is_none() {
                self.release_compare_and_set_count
                    .fetch_add(1, Ordering::Relaxed);
                let replacement = self
                    .release_contention_pins
                    .lock()
                    .unwrap_or_else(|_| panic!("test contention lock"))
                    .pop_front();
                if let Some(replacement) = replacement {
                    let mut persisted = self
                        .persisted_affinities
                        .lock()
                        .unwrap_or_else(|_| panic!("test pin lock"));
                    if let Some(current) = persisted.iter_mut().find(|pin| {
                        pin.provider() == affinity.provider()
                            && pin.session_id() == affinity.session_id()
                    }) {
                        *current = replacement;
                    } else {
                        persisted.push(replacement);
                    }
                    return Ok(false);
                }
            }

            let mut persisted_affinities = self
                .persisted_affinities
                .lock()
                .expect("test affinity store lock should not be poisoned");
            let existing_index = persisted_affinities.iter().position(|existing| {
                existing.provider() == affinity.provider()
                    && existing.session_id() == affinity.session_id()
            });
            let publication_time = affinity.last_seen_unix_seconds();
            let current_state_matches = match existing_index {
                None => expected_version == 0 && observation.active_account().is_none(),
                Some(index) => {
                    let existing = &persisted_affinities[index];
                    if existing.pin_version() != expected_version {
                        false
                    } else {
                        match observation.active_account() {
                            None => {
                                existing.account_id().is_none()
                                    || (!fake_pin_is_active_at(
                                        existing.last_seen_unix_seconds(),
                                        publication_time,
                                        pin_ttl_seconds,
                                    ) && existing.account_id().is_some())
                            }
                            Some(observed_account) => {
                                existing.account_id() == Some(observed_account)
                                    && fake_pin_is_active_at(
                                        existing.last_seen_unix_seconds(),
                                        publication_time,
                                        pin_ttl_seconds,
                                    )
                            }
                        }
                    }
                }
            };
            if !current_state_matches {
                return Ok(false);
            }

            let stored_last_seen = existing_index.map_or(publication_time, |index| {
                persisted_affinities[index]
                    .last_seen_unix_seconds()
                    .max(publication_time)
            });
            let replacement =
                    codex_router_state::session_account_affinity::SessionAccountAffinity::with_pin_state(
                        affinity.provider(),
                        affinity.session_id(),
                        affinity.account_id().cloned(),
                        desired_version,
                        stored_last_seen,
                    );
            if let Some(index) = existing_index {
                persisted_affinities[index] = replacement;
            } else {
                persisted_affinities.push(replacement);
            }
            Ok(true)
        })
    }

    fn load_session_account_affinity<'a>(
        &'a self,
        provider: Provider,
        session_id: &'a str,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Option<codex_router_state::session_account_affinity::SessionAccountAffinity>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async move {
            if let Some(affinity_read) = &self.affinity_read {
                affinity_read.entered.notify_one();
                affinity_read.release.notified().await;
            }
            let persisted_affinities = self
                .persisted_affinities
                .lock()
                .expect("test affinity store lock should not be poisoned");
            Ok(persisted_affinities
                .iter()
                .find(|affinity| {
                    affinity.provider() == provider && affinity.session_id() == session_id
                })
                .cloned())
        })
    }
}
