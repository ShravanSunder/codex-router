use super::*;

pub(super) fn quota_account(
    account_id: &str,
    remaining_headroom: u32,
    freshness: SnapshotFreshness,
) -> QuotaAwareAccountState {
    let account_id = match codex_router_core::ids::AccountId::new(account_id) {
        Ok(account_id) => account_id,
        Err(error) => panic!("account id should parse: {error}"),
    };
    QuotaAwareAccountState::new(account_id, remaining_headroom, freshness)
}

pub(super) fn persist_account_with_snapshot_and_token(
    state: &SqliteStateStore,
    secrets: &EncryptedCredentialStore,
    account: &AccountRecord,
    remaining_headroom: u32,
    upstream_token: &str,
) {
    let account_with_generation = account.clone().with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation) {
        panic!("account should persist: {error}");
    }
    let snapshot = PersistedQuotaSnapshot::new(
        account.account_id().clone(),
        QuotaSnapshotSource::MockEndpoint,
    )
    .with_observed_unix_seconds(1_000)
    .with_route_band("responses", remaining_headroom)
    .with_stale_penalty(false);
    if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(state, &snapshot) {
        panic!("quota snapshot should persist: {error}");
    }
    let selector_window = PersistedSelectorQuotaWindow::new(
        account.account_id().clone(),
        "responses",
        18_000,
        if remaining_headroom == 0 {
            SelectorQuotaWindowStatus::Ineligible
        } else {
            SelectorQuotaWindowStatus::Eligible
        },
    )
    .with_remaining_headroom(remaining_headroom)
    .with_effective(true)
    .with_observed_unix_seconds(test_unix_seconds())
    .with_reset_unix_seconds(selector_reset_seconds(18_000));
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(state, &selector_window) {
        panic!("selector quota window should persist: {error}");
    }
    let weekly_selector_window = PersistedSelectorQuotaWindow::new(
        account.account_id().clone(),
        "responses",
        604_800,
        if remaining_headroom == 0 {
            SelectorQuotaWindowStatus::Ineligible
        } else {
            SelectorQuotaWindowStatus::Eligible
        },
    )
    .with_remaining_headroom(remaining_headroom)
    .with_effective(false)
    .with_observed_unix_seconds(test_unix_seconds())
    .with_reset_unix_seconds(selector_reset_seconds(604_800));
    if let Err(error) =
        SelectorQuotaRepository::upsert_selector_window(state, &weekly_selector_window)
    {
        panic!("weekly selector quota window should persist: {error}");
    }
    let token_key = match openai_account_credential_bundle_key(account.account_id(), 1) {
        Ok(token_key) => token_key,
        Err(error) => panic!("token key should build: {error}"),
    };
    let bundle = match AccountCredentialBundle::imported_codex_auth(
        upstream_token,
        Some(format!("{upstream_token}-refresh")),
    )
    .to_secret_string()
    {
        Ok(bundle) => bundle,
        Err(error) => panic!("credential bundle should serialize: {error}"),
    };
    if let Err(error) = secrets.write_secret(&token_key, &bundle) {
        panic!("upstream token should persist: {error}");
    }
}

pub(super) async fn set_weekly_floor_for_test(
    database_path: &Path,
    account_label: &str,
    floor_basis_points: u16,
) {
    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(database_path)
        .await
        .expect("weekly-floor mutation store should open");
    mutation
        .set_weekly_quota_floor_by_label(
            account_label,
            Some(
                WeeklyQuotaFloorBasisPoints::new(floor_basis_points)
                    .expect("weekly floor should validate"),
            ),
        )
        .await
        .expect("weekly floor should commit");
    mutation.close().await;
}

pub(super) async fn seed_computable_weekly_margin_for_test(
    database_path: &Path,
    account_id: &AccountId,
    now_unix_seconds: u64,
    history_span_seconds: u64,
    prior_remaining_percent: u32,
    current_remaining_percent: u32,
    weekly_reset_seconds: u64,
) {
    let history_start = now_unix_seconds.saturating_sub(history_span_seconds);
    let history_middle = history_start + history_span_seconds / 2;
    let weekly_reset = now_unix_seconds + weekly_reset_seconds;
    let state = SqliteStateStore::open(database_path).expect("state should reopen for windows");
    let windows = [
        PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(90)
        .with_effective(true)
        .with_observed_unix_seconds(now_unix_seconds)
        .with_reset_unix_seconds(now_unix_seconds + 4 * 3_600),
        PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(current_remaining_percent)
        .with_effective(false)
        .with_observed_unix_seconds(now_unix_seconds)
        .with_reset_unix_seconds(weekly_reset),
    ];
    SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        &state,
        account_id,
        "responses",
        &windows,
        now_unix_seconds,
        now_unix_seconds + 300,
    )
    .expect("fresh selector windows should persist atomically");
    drop(state);

    let state = AsyncSqliteStateStore::open(database_path)
        .await
        .expect("async state should open for history");
    append_history_series_with_reset(
        &state,
        account_id,
        "responses",
        604_800,
        weekly_reset,
        &[
            (history_start, prior_remaining_percent),
            (history_middle, prior_remaining_percent),
            (now_unix_seconds, current_remaining_percent),
        ],
    )
    .await;
    record_completed_active_session(
        &state,
        account_id,
        &format!("process-floor-margin-{}", account_id.as_str()),
        &format!("reservation-floor-margin-{}", account_id.as_str()),
        history_start,
        now_unix_seconds,
    )
    .await;
    state
        .refresh_active_session_rollups_for_interval(
            "responses",
            history_start,
            now_unix_seconds,
            300,
        )
        .await
        .expect("active-session rollups should refresh");
    state.close().await.expect("async state should close");
}

pub(super) async fn assert_bad_projected_margin_does_not_floor_block_for_test(
    database_path: &Path,
    account_id: &AccountId,
    now_unix_seconds: u64,
    floor_basis_points: i64,
) {
    let state = AsyncSqliteStateStore::open(database_path)
        .await
        .expect("async state should open for margin assertion");
    let projection = project_route_band_selection_inputs_with_active_counts(
        &state,
        "responses",
        now_unix_seconds,
        60,
        None,
    )
    .await
    .expect("weekly-margin projection should load");
    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        RESPONSES_HTTP.clone(),
        projection.accounts().to_vec(),
    ));
    let account = assessment
        .accounts()
        .iter()
        .find(|account| account.account_id() == account_id)
        .expect("weekly-margin account should be assessed");
    assert!(
        account
            .weekly_survival_margin_basis_points()
            .is_some_and(|margin| margin < floor_basis_points),
        "test requires a forecast below the configured floor: {account:?}"
    );
    assert_eq!(
        account.routing_exclusion(),
        codex_router_selection::burn_down::RoutingExclusion::None,
        "forecast must not control the observed-current floor: {account:?}"
    );
    assert_eq!(assessment.preferred_next(), Some(account.account_id()));
    state.close().await.expect("assertion state should close");
}

pub(super) async fn append_history_series_with_reset(
    state: &AsyncSqliteStateStore,
    account_id: &AccountId,
    route_band: &str,
    limit_window_seconds: u64,
    reset_unix_seconds: u64,
    samples: &[(u64, u32)],
) {
    for (observed_unix_seconds, remaining_headroom) in samples {
        let observation = PersistedQuotaHistoryObservation::new(
            account_id.clone(),
            account_id.as_str(),
            route_band,
            limit_window_seconds,
            *observed_unix_seconds,
            *remaining_headroom,
        )
        .with_window_status(SelectorQuotaWindowStatus::Eligible)
        .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
        .with_refresh_outcome(QuotaHistoryRefreshOutcome::Success)
        .with_reset_unix_seconds(reset_unix_seconds);
        if let Err(error) =
            AsyncQuotaHistoryRepository::append_quota_history_observation(state, &observation).await
        {
            panic!("quota history should append: {error}");
        }
    }
}

pub(super) async fn record_completed_active_session(
    state: &AsyncSqliteStateStore,
    account_id: &AccountId,
    process_run_id: &str,
    reservation_id: &str,
    acquired_unix_seconds: u64,
    released_unix_seconds: u64,
) {
    let reservation_id = ReservationId::new(reservation_id);
    if let Err(error) = state
        .record_active_client_acquired(
            "responses",
            process_run_id,
            &reservation_id,
            account_id,
            acquired_unix_seconds,
            1,
        )
        .await
    {
        panic!("historical active session should acquire: {error}");
    }
    if let Err(error) = state
        .record_active_client_released(
            "responses",
            process_run_id,
            &reservation_id,
            released_unix_seconds,
        )
        .await
    {
        panic!("historical active session should release: {error}");
    }
}

pub(super) const fn hours_minutes(hours: u64, minutes: u64) -> u64 {
    hours * 3_600 + minutes * 60
}

pub(super) fn persist_account_with_selector_window_reset_specs(
    state: &SqliteStateStore,
    account: &AccountRecord,
    route_band: &str,
    windows: &[(u64, u32, bool, u64)],
) {
    let account_with_generation = account.clone().with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation) {
        panic!("account should persist: {error}");
    }
    for (limit_window_seconds, remaining_headroom, effective, reset_unix_seconds) in windows {
        let selector_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            route_band,
            *limit_window_seconds,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(*remaining_headroom)
        .with_effective(*effective)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(*reset_unix_seconds);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(state, &selector_window)
        {
            panic!("selector quota window should persist: {error}");
        }
    }
}

pub(super) fn persist_account_with_selector_windows(
    state: &SqliteStateStore,
    account: &AccountRecord,
    route_bands: &[&str],
    remaining_headroom: u32,
) {
    let account_with_generation = account.clone().with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation) {
        panic!("account should persist: {error}");
    }
    for route_band in route_bands {
        let selector_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            *route_band,
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(remaining_headroom)
        .with_effective(true)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(18_000));
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(state, &selector_window)
        {
            panic!("selector quota window should persist: {error}");
        }
        let weekly_selector_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            *route_band,
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(remaining_headroom)
        .with_effective(false)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(604_800));
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(state, &weekly_selector_window)
        {
            panic!("weekly selector quota window should persist: {error}");
        }
    }
}

pub(super) fn persist_account_with_selector_window_specs(
    state: &SqliteStateStore,
    account: &AccountRecord,
    route_band: &str,
    windows: &[(u64, u32, bool)],
) {
    let account_with_generation = account.clone().with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation) {
        panic!("account should persist: {error}");
    }
    for (limit_window_seconds, remaining_headroom, effective) in windows {
        let selector_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            route_band,
            *limit_window_seconds,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(*remaining_headroom)
        .with_effective(*effective)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(*limit_window_seconds));
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(state, &selector_window)
        {
            panic!("selector quota window should persist: {error}");
        }
    }
}

pub(super) fn refresh_served_floor_windows_for_test(
    state: &SqliteStateStore,
    account: &AccountRecord,
    observed_unix_seconds: u64,
    remaining_headroom: u32,
) {
    let windows = [
        PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(remaining_headroom)
        .with_effective(true)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_reset_unix_seconds(observed_unix_seconds + 18_000),
        PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(remaining_headroom)
        .with_effective(false)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_reset_unix_seconds(observed_unix_seconds + 604_800),
    ];
    SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        state,
        account.account_id(),
        "responses",
        &windows,
        observed_unix_seconds,
        observed_unix_seconds + 300,
    )
    .expect("served floor windows should be fresh");
}

pub(super) fn persist_fresh_account_with_selector_window_specs(
    state: &SqliteStateStore,
    account: &AccountRecord,
    route_band: &str,
    windows: &[(u64, u32, bool)],
) {
    let account_with_generation = account.clone().with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation) {
        panic!("account should persist: {error}");
    }
    let observed_unix_seconds = test_unix_seconds();
    let selector_windows = windows
        .iter()
        .map(|(limit_window_seconds, remaining_headroom, effective)| {
            PersistedSelectorQuotaWindow::new(
                account.account_id().clone(),
                route_band,
                *limit_window_seconds,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(*remaining_headroom)
            .with_effective(*effective)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_reset_unix_seconds(observed_unix_seconds.saturating_add(*limit_window_seconds))
        })
        .collect::<Vec<_>>();
    if let Err(error) = SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        state,
        account.account_id(),
        route_band,
        &selector_windows,
        observed_unix_seconds,
        observed_unix_seconds.saturating_add(300),
    ) {
        panic!("fresh selector quota windows should persist: {error}");
    }
}

pub(super) fn persist_account_with_selector_window_status_specs(
    state: &SqliteStateStore,
    account: &AccountRecord,
    route_band: &str,
    windows: &[(u64, u32, bool, SelectorQuotaWindowStatus)],
) {
    let account_with_generation = account.clone().with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation) {
        panic!("account should persist: {error}");
    }
    for (limit_window_seconds, remaining_headroom, effective, status) in windows {
        let selector_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            route_band,
            *limit_window_seconds,
            *status,
        )
        .with_remaining_headroom(*remaining_headroom)
        .with_effective(*effective)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(*limit_window_seconds));
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(state, &selector_window)
        {
            panic!("selector quota window should persist: {error}");
        }
    }
}
