//! SQLite-backed metadata boundary for codex-router.

pub mod account;
pub mod affinity_owner;
pub mod quota_snapshot;
pub mod repositories;
pub mod sqlite;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-state"
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use codex_router_core::affinity::AffinityKeyHash;
    use codex_router_core::ids::AccountId;
    use codex_router_core::ids::AffinityKey;
    use codex_router_core::routes::RouteBand;
    use rusqlite::Connection;

    use super::package_name;
    use crate::account::AccountRecord;
    use crate::account::AccountStatus;
    use crate::affinity_owner::AffinitySourceTransport;
    use crate::affinity_owner::PreviousResponseAffinityOwnerLookup;
    use crate::affinity_owner::PreviousResponseAffinityOwnerRecord;
    use crate::quota_snapshot::PersistedQuotaSnapshot;
    use crate::quota_snapshot::PersistedSelectorQuotaWindow;
    use crate::quota_snapshot::QuotaRefreshErrorClass;
    use crate::quota_snapshot::QuotaRefreshStatusSource;
    use crate::quota_snapshot::QuotaSnapshotSource;
    use crate::quota_snapshot::SelectorQuotaWindowStatus;
    use crate::repositories::AccountStateRepository;
    use crate::repositories::AffinityRepository;
    use crate::repositories::QuotaSnapshotRepository;
    use crate::repositories::SelectorQuotaRepository;
    use crate::sqlite::SqliteStateStore;
    use crate::sqlite::StateStoreError;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-state");
    }

    #[test]
    fn sqlite_migration_roundtrips_account_and_quota_snapshot() {
        let temp_dir = TestTempDir::new("migration_roundtrip");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };

        assert_eq!(store.schema_version(), 7);

        let account_id = match AccountId::new("acct_primary") {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        };
        let account = AccountRecord::new(account_id.clone(), "primary", AccountStatus::Enabled);
        if let Err(error) = store.upsert_account(&account) {
            panic!("account should persist: {error}");
        }

        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(100)
                .with_route_band("responses", 42)
                .with_reset_unix_seconds(160)
                .with_reset_credits_available(1)
                .with_stale_penalty(false);
        if let Err(error) = store.upsert_quota_snapshot(&snapshot) {
            panic!("quota snapshot should persist: {error}");
        }

        assert_eq!(store.load_account(&account_id), Ok(Some(account)));
        assert_eq!(store.load_quota_snapshot(&account_id), Ok(Some(snapshot)));
    }

    #[test]
    fn quota_snapshots_are_partitioned_by_route_band_for_one_account() {
        let temp_dir = TestTempDir::new("quota_route_band_partition");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_route_partition");
        let responses_snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 90);
        let models_snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_010)
                .with_route_band("models", 7);

        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &responses_snapshot) {
            panic!("responses quota snapshot should persist: {error}");
        }
        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &models_snapshot) {
            panic!("models quota snapshot should persist: {error}");
        }

        assert_eq!(
            QuotaSnapshotRepository::load_snapshot_for_route_band(&store, &account_id, "responses"),
            Ok(Some(responses_snapshot))
        );
        assert_eq!(
            QuotaSnapshotRepository::load_snapshot_for_route_band(&store, &account_id, "models"),
            Ok(Some(models_snapshot))
        );
    }

    #[test]
    fn selector_input_reads_durable_per_window_rows_without_status_renderer() {
        let temp_dir = TestTempDir::new("selector_input_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_selector_windows");
        let account = AccountRecord::new(account_id.clone(), "selector", AccountStatus::Enabled)
            .with_active_credential_generation(3);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Stale,
        )
        .with_remaining_headroom(72)
        .with_reset_unix_seconds(19_000)
        .with_observed_unix_seconds(1_000);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Stale,
        )
        .with_remaining_headroom(41)
        .with_reset_unix_seconds(700_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &short_window) {
            panic!("short window should persist: {error}");
        }
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &weekly_window)
        {
            panic!("weekly window should persist: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };

        assert_eq!(selector_inputs.len(), 1);
        let input = &selector_inputs[0];
        assert_eq!(input.account_id(), &account_id);
        assert_eq!(input.account_label(), "selector");
        assert_eq!(input.account_status(), AccountStatus::Enabled);
        assert_eq!(input.active_credential_generation(), Some(3));
        assert_eq!(input.route_band(), "responses");
        assert_eq!(input.windows(), &[weekly_window, short_window]);
        let effective = input
            .windows()
            .iter()
            .find(|window| window.effective())
            .unwrap_or_else(|| panic!("effective selector window should exist"));
        assert_eq!(effective.limit_window_seconds(), 604_800);
        assert_eq!(effective.status(), SelectorQuotaWindowStatus::Stale);
        assert_eq!(effective.remaining_headroom(), 41);
        assert_eq!(effective.reset_unix_seconds(), Some(700_000));
    }

    #[test]
    fn refresh_success_replaces_selector_windows_and_records_status() {
        let temp_dir = TestTempDir::new("refresh_success_replaces_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_refresh_success");
        let account = AccountRecord::new(account_id.clone(), "refresh", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let old_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(99)
        .with_observed_unix_seconds(900);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &old_window) {
            panic!("old selector window should persist: {error}");
        }
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(72)
        .with_reset_unix_seconds(19_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(44)
        .with_reset_unix_seconds(700_000)
        .with_observed_unix_seconds(1_000);

        if let Err(error) =
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &store,
                &account_id,
                "responses",
                &[short_window.clone(), weekly_window.clone()],
                1_000,
                2_000,
            )
        {
            panic!("refresh success should persist atomically: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_500,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        assert_eq!(selector_inputs[0].windows(), &[short_window, weekly_window]);
        let statuses = match SelectorQuotaRepository::quota_refresh_statuses_for_route_band(
            &store,
            "responses",
        ) {
            Ok(statuses) => statuses,
            Err(error) => panic!("refresh statuses should load: {error}"),
        };
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].account_id(), &account_id);
        assert_eq!(
            statuses[0].status_source(),
            QuotaRefreshStatusSource::Recorded
        );
        assert_eq!(statuses[0].last_success_unix_seconds(), Some(1_000));
        assert_eq!(statuses[0].last_attempt_unix_seconds(), Some(1_000));
        assert_eq!(statuses[0].last_error_class(), None);
        assert_eq!(statuses[0].stale_after_unix_seconds(), Some(2_000));
    }

    #[test]
    fn refresh_failure_preserves_windows_and_overlays_stale_on_read() {
        let temp_dir = TestTempDir::new("refresh_failure_preserves_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_refresh_failure");
        let account = AccountRecord::new(account_id.clone(), "failure", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(61)
        .with_reset_unix_seconds(19_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        if let Err(error) =
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &store,
                &account_id,
                "responses",
                std::slice::from_ref(&short_window),
                1_000,
                10_000,
            )
        {
            panic!("refresh success seed should persist: {error}");
        }

        if let Err(error) =
            SelectorQuotaRepository::record_refresh_failure_preserving_selector_windows(
                &store,
                &account_id,
                "responses",
                2_000,
                QuotaRefreshErrorClass::NetworkError,
            )
        {
            panic!("refresh failure should preserve windows: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            2_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        let windows = selector_inputs[0].windows();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Stale);
        assert_eq!(windows[0].remaining_headroom(), 61);
        assert_eq!(windows[0].reset_unix_seconds(), Some(19_000));
        assert_eq!(windows[0].observed_unix_seconds(), 1_000);
        let statuses = match SelectorQuotaRepository::quota_refresh_statuses_for_route_band(
            &store,
            "responses",
        ) {
            Ok(statuses) => statuses,
            Err(error) => panic!("refresh statuses should load: {error}"),
        };
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].last_success_unix_seconds(), Some(1_000));
        assert_eq!(statuses[0].last_attempt_unix_seconds(), Some(2_000));
        assert_eq!(
            statuses[0].last_error_class(),
            Some(QuotaRefreshErrorClass::NetworkError)
        );
        assert_eq!(statuses[0].stale_after_unix_seconds(), Some(2_000));
    }

    #[test]
    fn legacy_selector_rows_without_refresh_status_are_stale_and_reported() {
        let temp_dir = TestTempDir::new("legacy_missing_refresh_status");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let alpha_id = account_id("acct_alpha_legacy");
        let beta_id = account_id("acct_beta_empty");
        if let Err(error) = AccountStateRepository::upsert_account(
            &store,
            &AccountRecord::new(alpha_id.clone(), "alpha", AccountStatus::Enabled),
        ) {
            panic!("alpha account should persist: {error}");
        }
        if let Err(error) = AccountStateRepository::upsert_account(
            &store,
            &AccountRecord::new(beta_id, "beta", AccountStatus::Enabled),
        ) {
            panic!("beta account should persist: {error}");
        }
        let legacy_window = PersistedSelectorQuotaWindow::new(
            alpha_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(88)
        .with_reset_unix_seconds(20_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &legacy_window)
        {
            panic!("legacy selector window should persist: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_500,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 2);
        let alpha = selector_inputs
            .iter()
            .find(|input| input.account_id() == &alpha_id)
            .unwrap_or_else(|| panic!("alpha selector input should exist"));
        assert_eq!(alpha.windows().len(), 1);
        assert_eq!(
            alpha.windows()[0].status(),
            SelectorQuotaWindowStatus::Stale
        );
        assert_eq!(alpha.windows()[0].remaining_headroom(), 88);
        let statuses = match SelectorQuotaRepository::quota_refresh_statuses_for_route_band(
            &store,
            "responses",
        ) {
            Ok(statuses) => statuses,
            Err(error) => panic!("refresh statuses should load: {error}"),
        };
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].account_id(), &alpha_id);
        assert_eq!(
            statuses[0].status_source(),
            QuotaRefreshStatusSource::LegacyMissingRefreshStatus
        );
        assert_eq!(statuses[0].last_success_unix_seconds(), None);
        assert_eq!(statuses[0].last_attempt_unix_seconds(), None);
        assert_eq!(statuses[0].last_error_class(), None);
        assert_eq!(statuses[0].stale_after_unix_seconds(), None);
    }

    #[test]
    fn v2_migration_backfills_selector_windows_from_existing_quota_snapshots() {
        let temp_dir = TestTempDir::new("v2_selector_backfill");
        let database_path = temp_dir.path().join("state.sqlite");
        create_v2_database_with_quota_snapshot(&database_path);

        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("v2 state store should migrate to current schema: {error}"),
        };

        assert_eq!(store.schema_version(), 7);
        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load after migration: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        let input = &selector_inputs[0];
        assert_eq!(input.account_id(), &account_id("acct_v2_backfill"));
        assert_eq!(input.active_credential_generation(), Some(1));
        assert_eq!(input.windows().len(), 1);
        let window = &input.windows()[0];
        assert_eq!(window.status(), SelectorQuotaWindowStatus::Stale);
        assert_eq!(window.remaining_headroom(), 64);
        assert_eq!(window.reset_unix_seconds(), Some(2_000));
        assert_eq!(window.limit_window_seconds(), 18_000);
        assert!(window.effective());
        let expected_code_review_snapshot = PersistedQuotaSnapshot::new(
            account_id("acct_v2_backfill"),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(1_000)
        .with_route_band("code_review", 64)
        .with_reset_unix_seconds(2_000)
        .with_stale_penalty(false);
        assert_eq!(
            QuotaSnapshotRepository::load_snapshot_for_route_band(
                &store,
                &account_id("acct_v2_backfill"),
                "code_review"
            ),
            Ok(Some(expected_code_review_snapshot))
        );
        let code_review_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "code_review",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
        assert_eq!(code_review_inputs.len(), 1);
        assert!(code_review_inputs[0].windows().is_empty());
    }

    #[test]
    fn v3_migration_removes_legacy_code_review_selector_windows() {
        let temp_dir = TestTempDir::new("v3_code_review_selector_cleanup");
        let database_path = temp_dir.path().join("state.sqlite");
        create_v3_database_with_legacy_code_review_selector_window(&database_path);

        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("v3 state store should migrate to current schema: {error}"),
        };

        assert_eq!(store.schema_version(), 7);
        let responses_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("responses selector input should load: {error}"),
        };
        assert_eq!(responses_inputs.len(), 1);
        assert_eq!(responses_inputs[0].windows().len(), 1);
        let code_review_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "code_review",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
        assert_eq!(code_review_inputs.len(), 1);
        assert!(code_review_inputs[0].windows().is_empty());
    }

    #[test]
    fn credential_mutation_invalidates_response_backed_alias_family_atomically() {
        let temp_dir = TestTempDir::new("credential_mutation_invalidates_aliases");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_credential_mutation");
        let account = AccountRecord::new(account_id.clone(), "mutation", AccountStatus::Disabled);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        for route_band in [
            "responses",
            "models",
            "memories_trace_summarize",
            "responses_compact",
            "code_review",
        ] {
            let snapshot =
                PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                    .with_observed_unix_seconds(3_000)
                    .with_route_band(route_band, 88)
                    .with_reset_unix_seconds(4_000)
                    .with_stale_penalty(false);
            if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &snapshot) {
                panic!("{route_band} snapshot should persist: {error}");
            }
        }
        let legacy_code_review_selector_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "code_review",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(77)
        .with_reset_unix_seconds(999_999)
        .with_effective(true)
        .with_observed_unix_seconds(12_345);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(
            &store,
            &legacy_code_review_selector_window,
        ) {
            panic!("legacy code_review selector window should persist: {error}");
        }

        if let Err(error) = store.activate_account_credential_generation_and_invalidate_quota(
            &account_id,
            7,
            AccountStatus::Enabled,
        ) {
            panic!("credential mutation should activate and invalidate atomically: {error}");
        }

        let loaded_account = match AccountStateRepository::load_account(&store, &account_id) {
            Ok(Some(account)) => account,
            Ok(None) => panic!("account should still exist"),
            Err(error) => panic!("account should load: {error}"),
        };
        assert_eq!(loaded_account.status(), AccountStatus::Enabled);
        assert_eq!(loaded_account.active_credential_generation(), Some(7));
        for route_band in [
            "responses",
            "models",
            "memories_trace_summarize",
            "responses_compact",
        ] {
            let snapshot = match QuotaSnapshotRepository::load_snapshot_for_route_band(
                &store,
                &account_id,
                route_band,
            ) {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => panic!("{route_band} stale marker should exist"),
                Err(error) => panic!("{route_band} stale marker should load: {error}"),
            };
            assert_eq!(snapshot.remaining_headroom(), 0);
            assert_eq!(snapshot.observed_unix_seconds(), 0);
            assert_eq!(snapshot.reset_unix_seconds(), None);
            assert!(snapshot.stale_penalty());
            let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
                &store, route_band, 3_000,
            ) {
                Ok(inputs) => inputs,
                Err(error) => {
                    panic!("{route_band} selector input should load after mutation: {error}")
                }
            };
            assert_eq!(selector_inputs.len(), 1);
            let windows = selector_inputs[0].windows();
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Ineligible);
            assert_eq!(windows[0].remaining_headroom(), 0);
            assert_eq!(windows[0].observed_unix_seconds(), 0);
            assert!(windows[0].effective());
        }
        let code_review_snapshot = match QuotaSnapshotRepository::load_snapshot_for_route_band(
            &store,
            &account_id,
            "code_review",
        ) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("code_review stale marker should exist"),
            Err(error) => panic!("code_review stale marker should load: {error}"),
        };
        assert_eq!(code_review_snapshot.remaining_headroom(), 0);
        assert!(code_review_snapshot.stale_penalty());
        let code_review_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "code_review",
            3_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
        assert_eq!(code_review_inputs.len(), 1);
        assert!(code_review_inputs[0].windows().is_empty());
    }

    #[test]
    fn credential_mutation_invalidates_selector_windows_atomically() {
        let temp_dir = TestTempDir::new("credential_mutation_invalidates_selector_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_selector_mutation");
        let account = AccountRecord::new(
            account_id.clone(),
            "selector-mutation",
            AccountStatus::Disabled,
        );
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let selector_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(50)
        .with_effective(true)
        .with_observed_unix_seconds(9_000);
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&store, &selector_window)
        {
            panic!("selector window should persist: {error}");
        }
        let weekly_selector_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(99)
        .with_reset_unix_seconds(700_000)
        .with_effective(true)
        .with_observed_unix_seconds(9_000);
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&store, &weekly_selector_window)
        {
            panic!("weekly selector window should persist: {error}");
        }

        if let Err(error) = store.activate_account_credential_generation_and_invalidate_quota(
            &account_id,
            2,
            AccountStatus::Enabled,
        ) {
            panic!("credential mutation should invalidate selector windows: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            9_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        let windows = selector_inputs[0].windows();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].limit_window_seconds(), 18_000);
        assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Ineligible);
        assert_eq!(windows[0].remaining_headroom(), 0);
        assert_eq!(windows[0].reset_unix_seconds(), None);
        assert_eq!(windows[0].observed_unix_seconds(), 0);
        assert!(windows[0].effective());
    }

    #[test]
    fn quota_snapshot_upsert_keeps_code_review_out_of_selector_projection() {
        let temp_dir = TestTempDir::new("code_review_status_only");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_code_review_status_only");
        let account = AccountRecord::new(account_id.clone(), "status-only", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(3_000)
                .with_route_band("code_review", 88)
                .with_reset_unix_seconds(4_000)
                .with_stale_penalty(false);

        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &snapshot) {
            panic!("code_review quota snapshot should persist: {error}");
        }

        assert_eq!(
            QuotaSnapshotRepository::load_snapshot_for_route_band(
                &store,
                &account_id,
                "code_review"
            ),
            Ok(Some(snapshot))
        );
        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "code_review",
            3_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        assert!(selector_inputs[0].windows().is_empty());
    }

    #[test]
    fn corrupt_account_metadata_fails_closed_for_that_account_only() {
        let temp_dir = TestTempDir::new("corrupt_account");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let healthy_id = match AccountId::new("acct_healthy") {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        };
        let corrupt_id = match AccountId::new("acct_corrupt") {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        };

        if let Err(error) = store.upsert_account(&AccountRecord::new(
            healthy_id.clone(),
            "healthy",
            AccountStatus::Enabled,
        )) {
            panic!("healthy account should persist: {error}");
        }
        if let Err(error) = store.insert_raw_account_for_test(
            corrupt_id.as_str(),
            "refresh-token-canary",
            "not-a-status",
        ) {
            panic!("corrupt fixture should persist: {error}");
        }

        let corrupt_error = match store.load_account(&corrupt_id) {
            Ok(account) => panic!("corrupt account should not load: {account:?}"),
            Err(error) => error,
        };

        assert!(matches!(
            corrupt_error,
            StateStoreError::CorruptAccount { .. }
        ));
        assert!(!format!("{corrupt_error:?}").contains("refresh-token-canary"));
        assert_eq!(
            store.load_account(&healthy_id),
            Ok(Some(AccountRecord::new(
                healthy_id,
                "healthy",
                AccountStatus::Enabled
            )))
        );
    }

    #[test]
    fn unsupported_schema_version_fails_closed_on_open() {
        let temp_dir = TestTempDir::new("unsupported_schema");
        let database_path = temp_dir.path().join("state.sqlite");
        let raw = match rusqlite::Connection::open(&database_path) {
            Ok(raw) => raw,
            Err(error) => panic!("raw sqlite should open: {error}"),
        };
        if let Err(error) = raw.pragma_update(None, "user_version", 999_i64) {
            panic!("schema fixture should persist: {error}");
        }
        drop(raw);

        let error = match SqliteStateStore::open(&database_path) {
            Ok(store) => panic!("unsupported schema should not open: {store:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            StateStoreError::UnsupportedSchemaVersion { version: 999 }
        );
    }

    #[test]
    fn state_repository_contracts_are_proxy_usable() {
        let temp_dir = TestTempDir::new("repository_contracts");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_contract");
        let account = AccountRecord::new(account_id.clone(), "contract", AccountStatus::Enabled);
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_200)
                .with_route_band("responses", 33);
        let affinity_key = AffinityKey::new("response_previous");

        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account repository should persist: {error}");
        }
        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &snapshot) {
            panic!("quota repository should persist: {error}");
        }
        if let Err(error) = AffinityRepository::pin_account(&store, &affinity_key, &account_id) {
            panic!("affinity repository should persist: {error}");
        }

        assert_eq!(
            AccountStateRepository::load_account(&store, &account_id),
            Ok(Some(account))
        );
        assert_eq!(
            QuotaSnapshotRepository::load_snapshot(&store, &account_id),
            Ok(Some(snapshot))
        );
        assert_eq!(
            AffinityRepository::load_pin(&store, &affinity_key),
            Ok(Some(account_id))
        );
    }

    #[test]
    fn previous_response_affinity_owner_repository_is_hash_only_and_route_scoped() {
        let temp_dir = TestTempDir::new("affinity_owner_hash_only");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let hash = affinity_hash('a');
        let responses_owner = PreviousResponseAffinityOwnerRecord::new(
            hash.clone(),
            account_id("acct_owner"),
            7,
            RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            1_000,
        );
        let models_owner = PreviousResponseAffinityOwnerRecord::new(
            hash.clone(),
            account_id("acct_models"),
            9,
            RouteBand::Models,
            AffinitySourceTransport::WebSocket,
            1_100,
        );

        if let Err(error) =
            AffinityRepository::write_previous_response_owner(&store, &responses_owner)
        {
            panic!("responses affinity owner should persist: {error}");
        }
        if let Err(error) = AffinityRepository::write_previous_response_owner(&store, &models_owner)
        {
            panic!("models affinity owner should persist: {error}");
        }

        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &hash,
                RouteBand::Responses.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Found(responses_owner))
        );
        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &hash,
                RouteBand::Models.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Found(models_owner))
        );
        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &affinity_hash('b'),
                RouteBand::Responses.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Missing)
        );
        assert_no_previous_response_id_in_affinity_owner_rows(&database_path, "resp_raw_canary");
    }

    #[test]
    fn previous_response_affinity_owner_detects_ambiguous_rows_and_can_purge() {
        let temp_dir = TestTempDir::new("affinity_owner_ambiguous_purge");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let hash = affinity_hash('c');
        for account_id_value in ["acct_first", "acct_second"] {
            let owner = PreviousResponseAffinityOwnerRecord::new(
                hash.clone(),
                account_id(account_id_value),
                1,
                RouteBand::Responses,
                AffinitySourceTransport::HttpSse,
                2_000,
            );
            if let Err(error) = AffinityRepository::write_previous_response_owner(&store, &owner) {
                panic!("affinity owner should persist: {error}");
            }
        }

        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &hash,
                RouteBand::Responses.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Ambiguous)
        );

        if let Err(error) = AffinityRepository::purge_previous_response_owners(&store) {
            panic!("affinity owners should purge: {error}");
        }
        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &hash,
                RouteBand::Responses.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Missing)
        );
    }

    #[test]
    fn state_repository_lists_accounts_in_selector_stable_order() {
        let temp_dir = TestTempDir::new("list_accounts");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let beta_id = account_id("acct_beta");
        let alpha_id = account_id("acct_alpha");
        let disabled_id = account_id("acct_disabled");
        let beta = AccountRecord::new(beta_id, "beta", AccountStatus::Enabled);
        let alpha = AccountRecord::new(alpha_id, "alpha", AccountStatus::Enabled);
        let disabled = AccountRecord::new(disabled_id, "disabled", AccountStatus::Disabled);

        if let Err(error) = AccountStateRepository::upsert_account(&store, &beta) {
            panic!("beta account should persist: {error}");
        }
        if let Err(error) = AccountStateRepository::upsert_account(&store, &alpha) {
            panic!("alpha account should persist: {error}");
        }
        if let Err(error) = AccountStateRepository::upsert_account(&store, &disabled) {
            panic!("disabled account should persist: {error}");
        }

        let accounts = match AccountStateRepository::list_accounts(&store) {
            Ok(accounts) => accounts,
            Err(error) => panic!("account repository should list accounts: {error}"),
        };

        assert_eq!(accounts, vec![alpha, beta, disabled]);
    }

    struct TestTempDir {
        path: PathBuf,
    }

    impl TestTempDir {
        fn new(name: &str) -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "codex-router-state-{name}-{}-{unique}",
                std::process::id()
            ));
            if let Err(error) = fs::create_dir(&path) {
                panic!(
                    "failed to create test directory {}: {error}",
                    path.display()
                );
            }

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestTempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                panic!(
                    "failed to remove test directory {}: {error}",
                    self.path.display()
                );
            }
        }
    }

    fn account_id(value: &str) -> AccountId {
        match AccountId::new(value) {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        }
    }

    fn affinity_hash(character: char) -> AffinityKeyHash {
        match AffinityKeyHash::new(character.to_string().repeat(64)) {
            Ok(hash) => hash,
            Err(error) => panic!("affinity hash should parse: {error}"),
        }
    }

    fn assert_no_previous_response_id_in_affinity_owner_rows(
        database_path: &Path,
        raw_previous_response_id: &str,
    ) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw sqlite should open: {error}"),
        };
        let count: i64 = match connection.query_row(
            "SELECT COUNT(*)
               FROM previous_response_affinity_owners
              WHERE affinity_key_hash = ?1
                 OR route_band = ?1
                 OR account_id = ?1
                 OR source_transport = ?1",
            [raw_previous_response_id],
            |row| row.get(0),
        ) {
            Ok(count) => count,
            Err(error) => panic!("raw sqlite count should query: {error}"),
        };
        assert_eq!(count, 0);
    }

    fn create_v2_database_with_quota_snapshot(database_path: &Path) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw v2 database should open: {error}"),
        };
        if let Err(error) = connection.execute_batch(
            "
            CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
            );

            CREATE TABLE quota_snapshots (
                account_id TEXT NOT NULL,
                source TEXT NOT NULL,
                observed_unix_seconds INTEGER NOT NULL,
                route_band TEXT NOT NULL,
                remaining_headroom INTEGER NOT NULL,
                reset_unix_seconds INTEGER,
                stale_penalty INTEGER NOT NULL,
                PRIMARY KEY (account_id, route_band)
            );

            CREATE TABLE affinity_pins (
                affinity_key TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL
            );

            INSERT INTO accounts (
                account_id, label, status, active_credential_generation
            ) VALUES (
                'acct_v2_backfill', 'v2-backfill', 'enabled', 1
            );

            INSERT INTO quota_snapshots (
                account_id, source, observed_unix_seconds, route_band,
                remaining_headroom, reset_unix_seconds, stale_penalty
            ) VALUES (
                'acct_v2_backfill', 'mock_endpoint', 1000, 'responses',
                64, 2000, 0
            );

            INSERT INTO quota_snapshots (
                account_id, source, observed_unix_seconds, route_band,
                remaining_headroom, reset_unix_seconds, stale_penalty
            ) VALUES (
                'acct_v2_backfill', 'mock_endpoint', 1000, 'code_review',
                64, 2000, 0
            );

            PRAGMA user_version = 2;
            ",
        ) {
            panic!("raw v2 database should initialize: {error}");
        }
    }

    fn create_v3_database_with_legacy_code_review_selector_window(database_path: &Path) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw v3 database should open: {error}"),
        };
        if let Err(error) = connection.execute_batch(
            "
            CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
            );

            CREATE TABLE quota_snapshots (
                account_id TEXT NOT NULL,
                source TEXT NOT NULL,
                observed_unix_seconds INTEGER NOT NULL,
                route_band TEXT NOT NULL,
                remaining_headroom INTEGER NOT NULL,
                reset_unix_seconds INTEGER,
                stale_penalty INTEGER NOT NULL,
                PRIMARY KEY (account_id, route_band)
            );

            CREATE TABLE affinity_pins (
                affinity_key TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL
            );

            CREATE TABLE selector_quota_windows (
                account_id TEXT NOT NULL,
                route_band TEXT NOT NULL,
                limit_window_seconds INTEGER NOT NULL,
                status TEXT NOT NULL,
                remaining_headroom INTEGER NOT NULL,
                reset_unix_seconds INTEGER,
                effective INTEGER NOT NULL,
                observed_unix_seconds INTEGER NOT NULL,
                PRIMARY KEY (account_id, route_band, limit_window_seconds)
            );

            INSERT INTO accounts (
                account_id, label, status, active_credential_generation
            ) VALUES (
                'acct_v3_cleanup', 'v3-cleanup', 'enabled', 1
            );

            INSERT INTO selector_quota_windows (
                account_id, route_band, limit_window_seconds, status,
                remaining_headroom, reset_unix_seconds, effective,
                observed_unix_seconds
            ) VALUES (
                'acct_v3_cleanup', 'responses', 18000, 'eligible',
                64, 2000, 1, 1000
            );

            INSERT INTO selector_quota_windows (
                account_id, route_band, limit_window_seconds, status,
                remaining_headroom, reset_unix_seconds, effective,
                observed_unix_seconds
            ) VALUES (
                'acct_v3_cleanup', 'code_review', 604800, 'eligible',
                77, 999999, 1, 12345
            );

            PRAGMA user_version = 3;
            ",
        ) {
            panic!("raw v3 database should initialize: {error}");
        }
    }
}
