use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;

use crate::account::AccountRecord;
use crate::account::AccountStatus;
use crate::account_routing_policy::WeeklyQuotaFloorBasisPoints;
use crate::credential_maintenance::CredentialMaintenanceState;
use crate::quota_snapshot::SelectorCredentialMaintenance;
use crate::sqlite::AsyncWeeklyQuotaFloorMutationStore;

use super::*;

#[test]
fn projection_rejects_rollup_gap_inside_quota_interval() {
    let account = AccountId::new("acct_rollup_gap").unwrap_or_else(|error| panic!("{error}"));
    let rollups = vec![
        ActiveSessionRollup::new(account.clone(), "responses", 0, 300, 300, 1),
        ActiveSessionRollup::new(account.clone(), "responses", 600, 900, 300, 1),
    ];

    assert!(
        !active_session_rollups_cover_interval(&rollups, &account, 0, 900),
        "a missing middle rollup bucket must downgrade active-session history"
    );
}

fn credential_maintenance_projection_input(
    maintenance_generation: u64,
    maintenance_state: CredentialMaintenanceState,
) -> SelectorQuotaInput {
    SelectorQuotaInput::new(
        AccountId::new("acct_maintenance_projection")
            .unwrap_or_else(|error| panic!("test account id should be valid: {error}")),
        "Claude Maintenance",
        Provider::Claude,
        AccountStatus::Enabled,
        Some(2),
        "messages",
        Vec::new(),
    )
    .with_credential_maintenance(Some(SelectorCredentialMaintenance::new(
        maintenance_generation,
        maintenance_state,
    )))
}

#[test]
fn projection_maps_active_generation_reauth_and_unrefreshable_to_needs_login() {
    for maintenance_state in [
        CredentialMaintenanceState::ReauthRequired,
        CredentialMaintenanceState::Unrefreshable,
    ] {
        let input = credential_maintenance_projection_input(2, maintenance_state);
        let projected = selection_account_state_from_selector_input(&input, None, 1_000);

        assert_eq!(
            projected.restriction(),
            Some(&SelectionAccountRestriction::NeedsLogin),
            "active generation maintenance state {maintenance_state:?} requires login"
        );
        assert!(
            !active_credential_is_routable(&input),
            "the reauth-required active generation must not be selected"
        );
    }
}

#[test]
fn projection_ignores_reauth_state_for_stale_credential_generation() {
    let input =
        credential_maintenance_projection_input(1, CredentialMaintenanceState::ReauthRequired);
    let projected = selection_account_state_from_selector_input(&input, None, 1_000);

    assert_eq!(
        projected.restriction(),
        Some(&SelectionAccountRestriction::Available),
        "stale maintenance state must not restrict the newer active credential"
    );
    assert!(
        active_credential_is_routable(&input),
        "a stale maintenance generation must leave the active credential routable"
    );
}

#[tokio::test]
async fn read_only_projection_returns_error_when_active_count_snapshot_is_unavailable() {
    let state = ActiveCountUnavailableProjectionRepository::new(account_id("acct_snapshot"));

    let result =
        project_route_band_selection_inputs_read_only(&state, "responses", 1_000, 7_200).await;

    assert_eq!(
        result,
        Err(active_count_snapshot_unavailable()),
        "read-only active-count failure must propagate instead of becoming zero active sessions"
    );
}

#[tokio::test]
async fn read_only_projection_does_not_call_refresh_rollups_or_mutating_active_count_reader() {
    let account_id = account_id("acct_pure_projection");
    let state = ReadOnlyProjectionPurityRepository::new(account_id.clone());

    let projection =
        project_route_band_selection_inputs_read_only(&state, "responses", 1_000, 7_200)
            .await
            .unwrap_or_else(|error| panic!("read-only projection should succeed: {error}"));

    assert_eq!(
        state.mutating_active_count_reads.load(Ordering::SeqCst),
        0,
        "read-only projection must not invoke stale-cleanup active-count reads"
    );
    assert_eq!(
        state.read_only_active_count_reads.load(Ordering::SeqCst),
        1,
        "read-only projection should load active counts through the read-only repository method"
    );
    assert_eq!(
        state.rollup_refreshes.load(Ordering::SeqCst),
        0,
        "read-only projection must not refresh active-session rollups"
    );
    assert_eq!(
        state.rollup_reads.load(Ordering::SeqCst),
        1,
        "the fixture should reach rollup estimation instead of passing before the refresh boundary"
    );
    assert_eq!(
        state.policy_bulk_reads.load(Ordering::SeqCst),
        1,
        "projection should bulk-load account policies exactly once"
    );
    assert_eq!(
        projection.accounts()[0].current_active_sessions(),
        2,
        "projection should use the read-only active-count snapshot"
    );
    assert_eq!(projection.accounts()[0].account_id(), &account_id);
}

#[tokio::test]
async fn projection_preserves_account_provider_for_route_profile_filtering() {
    let account_id = account_id("acct_claude_projection");
    let state =
        ReadOnlyProjectionPurityRepository::new(account_id.clone()).with_provider(Provider::Claude);

    let projection =
        project_route_band_selection_inputs_read_only(&state, "claude_messages", 1_000, 7_200)
            .await
            .unwrap_or_else(|error| panic!("provider projection should succeed: {error}"));

    assert_eq!(projection.accounts().len(), 1);
    assert_eq!(projection.accounts()[0].provider(), Provider::Claude);
}

#[tokio::test]
async fn projection_bulk_policy_read_is_account_isolated_and_visible_after_commit() {
    let temp_dir = ProjectionTempDir::new("policy_projection_visibility");
    let database_path = temp_dir.path.join("state.sqlite");
    let state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    let protected_account_id = account_id("acct_projection_protected");
    let unprotected_account_id = account_id("acct_projection_unprotected");
    for (account_id, label) in [
        (&protected_account_id, "protected"),
        (&unprotected_account_id, "unprotected"),
    ] {
        state
            .upsert_account(&AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                label,
                AccountStatus::Enabled,
            ))
            .await
            .expect("account should persist");
        for route_band in ["responses", "models"] {
            state
                .upsert_selector_quota_window(
                    &PersistedSelectorQuotaWindow::new(
                        account_id.clone(),
                        route_band,
                        604_800,
                        SelectorQuotaWindowStatus::Eligible,
                    )
                    .with_remaining_headroom(50)
                    .with_reset_unix_seconds(10_000)
                    .with_observed_unix_seconds(1_000)
                    .with_effective(true),
                )
                .await
                .expect("selector window should persist");
        }
    }

    let before_commit =
        project_route_band_selection_inputs_read_only(&state, "responses", 1_000, 7_200)
            .await
            .expect("pre-commit projection should succeed");
    assert!(
        before_commit
            .accounts()
            .iter()
            .all(|account| account.weekly_quota_floor_basis_points().is_none())
    );

    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect("mutation store should open");
    mutation
        .set_weekly_quota_floor_by_label(
            "protected",
            Some(WeeklyQuotaFloorBasisPoints::new(500).expect("valid floor")),
        )
        .await
        .expect("policy should commit");
    mutation.close().await;

    for route_band in ["responses", "models"] {
        let projection =
            project_route_band_selection_inputs_read_only(&state, route_band, 1_000, 7_200)
                .await
                .expect("post-commit projection should succeed");
        let protected = projection
            .accounts()
            .iter()
            .find(|account| account.account_id() == &protected_account_id)
            .expect("protected account should project");
        let unprotected = projection
            .accounts()
            .iter()
            .find(|account| account.account_id() == &unprotected_account_id)
            .expect("unprotected account should project");
        assert_eq!(protected.weekly_quota_floor_basis_points(), Some(500));
        assert_eq!(unprotected.weekly_quota_floor_basis_points(), None);
    }
}

#[tokio::test]
async fn invalid_persisted_policy_error_fails_the_whole_projection() {
    let state =
        ReadOnlyProjectionPurityRepository::with_policy_error(account_id("acct_invalid_policy"));

    assert_eq!(
        project_route_band_selection_inputs_read_only(&state, "responses", 1_000, 7_200).await,
        Err(StateStoreError::CorruptAccountRoutingPolicy)
    );
    assert_eq!(state.policy_bulk_reads.load(Ordering::SeqCst), 1);
    assert_eq!(state.read_only_active_count_reads.load(Ordering::SeqCst), 0);
    assert_eq!(state.rollup_refreshes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn writable_projection_bulk_loads_policy_once_and_attaches_it() {
    let account_id = account_id("acct_writable_policy");
    let state = ReadOnlyProjectionPurityRepository::with_policy_floor(
        account_id.clone(),
        WeeklyQuotaFloorBasisPoints::new(700).expect("valid floor"),
    );

    let projection = project_route_band_selection_inputs(&state, "responses", 1_000, 7_200)
        .await
        .expect("writable projection should succeed");

    assert_eq!(state.policy_bulk_reads.load(Ordering::SeqCst), 1);
    assert_eq!(state.mutating_active_count_reads.load(Ordering::SeqCst), 1);
    assert_eq!(state.read_only_active_count_reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        projection.accounts()[0].weekly_quota_floor_basis_points(),
        Some(700)
    );
}

struct ProjectionTempDir {
    path: PathBuf,
}

impl ProjectionTempDir {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "codex-router-state-projection-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("projection temp dir should create");
        Self { path }
    }
}

impl Drop for ProjectionTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct ActiveCountUnavailableProjectionRepository {
    account_id: AccountId,
}

impl ActiveCountUnavailableProjectionRepository {
    fn new(account_id: AccountId) -> Self {
        Self { account_id }
    }
}

impl AsyncSelectionProjectionRepository for ActiveCountUnavailableProjectionRepository {
    fn list_account_routing_policies(
        &self,
    ) -> BoxFuture<'_, Result<Vec<AccountRoutingPolicy>, StateStoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn selector_inputs_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<SelectorQuotaInput>, StateStoreError>> {
        Box::pin(async move {
            let window = PersistedSelectorQuotaWindow::new(
                self.account_id.clone(),
                route_band,
                18_000,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(80)
            .with_observed_unix_seconds(now_unix_seconds)
            .with_effective(true);
            Ok(vec![SelectorQuotaInput::new(
                self.account_id.clone(),
                "safe-label",
                Provider::Openai,
                AccountStatus::Enabled,
                Some(1),
                route_band,
                vec![window],
            )])
        })
    }

    fn active_client_counts_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveClientCount>, StateStoreError>> {
        Box::pin(async { Err(active_count_snapshot_unavailable()) })
    }

    fn active_client_counts_for_route_band_read_only<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveClientCount>, StateStoreError>> {
        Box::pin(async { Err(active_count_snapshot_unavailable()) })
    }

    fn quota_history_observations_for_window<'a>(
        &'a self,
        _account_id: &'a AccountId,
        _route_band: &'a str,
        _limit_window_seconds: u64,
        _observed_from_unix_seconds: u64,
        _observed_to_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<PersistedQuotaHistoryObservation>, StateStoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn active_session_rollups_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveSessionRollup>, StateStoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn refresh_active_session_rollups_for_interval<'a>(
        &'a self,
        _route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
        _bucket_seconds: u64,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async { Ok(()) })
    }
}

struct ReadOnlyProjectionPurityRepository {
    account_id: AccountId,
    provider: Provider,
    mutating_active_count_reads: AtomicUsize,
    read_only_active_count_reads: AtomicUsize,
    rollup_refreshes: AtomicUsize,
    rollup_reads: AtomicUsize,
    policy_bulk_reads: AtomicUsize,
    policy_error: bool,
    policy_floor: Option<WeeklyQuotaFloorBasisPoints>,
    allow_mutating_active_count_read: bool,
}

impl ReadOnlyProjectionPurityRepository {
    fn new(account_id: AccountId) -> Self {
        Self {
            account_id,
            provider: Provider::Openai,
            mutating_active_count_reads: AtomicUsize::new(0),
            read_only_active_count_reads: AtomicUsize::new(0),
            rollup_refreshes: AtomicUsize::new(0),
            rollup_reads: AtomicUsize::new(0),
            policy_bulk_reads: AtomicUsize::new(0),
            policy_error: false,
            policy_floor: None,
            allow_mutating_active_count_read: false,
        }
    }

    fn with_policy_error(account_id: AccountId) -> Self {
        Self {
            policy_error: true,
            ..Self::new(account_id)
        }
    }

    fn with_provider(mut self, provider: Provider) -> Self {
        self.provider = provider;
        self
    }

    fn with_policy_floor(account_id: AccountId, policy_floor: WeeklyQuotaFloorBasisPoints) -> Self {
        Self {
            policy_floor: Some(policy_floor),
            allow_mutating_active_count_read: true,
            ..Self::new(account_id)
        }
    }
}

impl AsyncSelectionProjectionRepository for ReadOnlyProjectionPurityRepository {
    fn list_account_routing_policies(
        &self,
    ) -> BoxFuture<'_, Result<Vec<AccountRoutingPolicy>, StateStoreError>> {
        Box::pin(async move {
            self.policy_bulk_reads.fetch_add(1, Ordering::SeqCst);
            if self.policy_error {
                Err(StateStoreError::CorruptAccountRoutingPolicy)
            } else if let Some(policy_floor) = self.policy_floor {
                Ok(vec![AccountRoutingPolicy::new(
                    self.account_id.clone(),
                    policy_floor,
                )])
            } else {
                Ok(Vec::new())
            }
        })
    }

    fn selector_inputs_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<SelectorQuotaInput>, StateStoreError>> {
        Box::pin(async move {
            let window = PersistedSelectorQuotaWindow::new(
                self.account_id.clone(),
                route_band,
                18_000,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(80)
            .with_reset_unix_seconds(2_000)
            .with_observed_unix_seconds(now_unix_seconds)
            .with_effective(true);
            Ok(vec![SelectorQuotaInput::new(
                self.account_id.clone(),
                "safe-label",
                self.provider,
                AccountStatus::Enabled,
                Some(1),
                route_band,
                vec![window],
            )])
        })
    }

    fn active_client_counts_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveClientCount>, StateStoreError>> {
        Box::pin(async move {
            self.mutating_active_count_reads
                .fetch_add(1, Ordering::SeqCst);
            if self.allow_mutating_active_count_read {
                Ok(vec![ActiveClientCount::new(self.account_id.clone(), 2, 2)])
            } else {
                Err(StateStoreError::Sqlite {
                    message: "read-only projection called mutating active-count reader".to_owned(),
                })
            }
        })
    }

    fn active_client_counts_for_route_band_read_only<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveClientCount>, StateStoreError>> {
        Box::pin(async move {
            self.read_only_active_count_reads
                .fetch_add(1, Ordering::SeqCst);
            Ok(vec![ActiveClientCount::new(self.account_id.clone(), 2, 2)])
        })
    }

    fn quota_history_observations_for_window<'a>(
        &'a self,
        _account_id: &'a AccountId,
        route_band: &'a str,
        limit_window_seconds: u64,
        _observed_from_unix_seconds: u64,
        _observed_to_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<PersistedQuotaHistoryObservation>, StateStoreError>> {
        Box::pin(async move {
            Ok(vec![
                PersistedQuotaHistoryObservation::new(
                    self.account_id.clone(),
                    "safe-label",
                    route_band,
                    limit_window_seconds,
                    700,
                    90,
                )
                .with_reset_unix_seconds(2_000),
                PersistedQuotaHistoryObservation::new(
                    self.account_id.clone(),
                    "safe-label",
                    route_band,
                    limit_window_seconds,
                    1_000,
                    80,
                )
                .with_reset_unix_seconds(2_000),
            ])
        })
    }

    fn active_session_rollups_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveSessionRollup>, StateStoreError>> {
        Box::pin(async move {
            self.rollup_reads.fetch_add(1, Ordering::SeqCst);
            Ok(vec![ActiveSessionRollup::new(
                self.account_id.clone(),
                route_band,
                700,
                1_000,
                300,
                2,
            )])
        })
    }

    fn refresh_active_session_rollups_for_interval<'a>(
        &'a self,
        _route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
        _bucket_seconds: u64,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async move {
            self.rollup_refreshes.fetch_add(1, Ordering::SeqCst);
            if self.allow_mutating_active_count_read {
                Ok(())
            } else {
                Err(StateStoreError::Sqlite {
                    message: "read-only projection called rollup refresh".to_owned(),
                })
            }
        })
    }
}

fn active_count_snapshot_unavailable() -> StateStoreError {
    StateStoreError::Sqlite {
        message: "active count snapshot unavailable".to_owned(),
    }
}

fn account_id(value: &str) -> AccountId {
    AccountId::new(value).unwrap_or_else(|error| panic!("test account id should parse: {error}"))
}
