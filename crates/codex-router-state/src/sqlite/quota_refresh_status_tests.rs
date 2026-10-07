use super::*;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::WindowKind;
use codex_router_core::routes::RouteBand;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::account::{AccountRecord, AccountStatus};
use crate::quota_snapshot::{
    PersistedQuotaSnapshot, PersistedSelectorQuotaWindow, QuotaRefreshErrorClass,
    SelectorQuotaWindowStatus,
};
use crate::window_observation::{
    WindowObservation, WindowObservationProps, WindowRejection, WindowRejectionProps,
};

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct QuotaStatusDirectory(PathBuf);

impl QuotaStatusDirectory {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "quota-refresh-status-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).expect("test directory");
        Self(directory)
    }
}

impl Drop for QuotaStatusDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove synthetic test directory");
    }
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct StoredSelectorWindow {
    route_band: String,
    limit_window_seconds: i64,
    status: String,
    remaining_headroom: i64,
    reset_unix_seconds: Option<i64>,
    effective: i64,
    observed_unix_seconds: i64,
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct StoredRouteBandState {
    route_band: String,
    state: String,
    reason_code: String,
    observed_unix_seconds: i64,
    expires_unix_seconds: i64,
}

#[derive(Debug, PartialEq)]
struct PreservedAccountState {
    selector_windows: Vec<StoredSelectorWindow>,
    route_states: Vec<StoredRouteBandState>,
    observations: Vec<WindowObservation>,
    rejections: Vec<WindowRejection>,
    account: Option<AccountRecord>,
    snapshot: Option<PersistedQuotaSnapshot>,
}

async fn read_preserved_account_state(
    state: &AsyncSqliteStateStore,
    account_id: &AccountId,
) -> PreservedAccountState {
    PreservedAccountState {
        selector_windows: sqlx::query_as(
            "SELECT route_band, limit_window_seconds, status, remaining_headroom,
                    reset_unix_seconds, effective, observed_unix_seconds
               FROM selector_quota_windows WHERE account_id = ?1
              ORDER BY route_band, limit_window_seconds",
        )
        .bind(account_id.as_str())
        .fetch_all(&state.pool)
        .await
        .expect("stored selector rows"),
        route_states: sqlx::query_as(
            "SELECT route_band, state, reason_code, observed_unix_seconds, expires_unix_seconds
               FROM route_band_account_states WHERE account_id = ?1 ORDER BY route_band",
        )
        .bind(account_id.as_str())
        .fetch_all(&state.pool)
        .await
        .expect("stored exhaustion rows"),
        observations: state
            .window_observations_for_account(account_id)
            .await
            .expect("observations"),
        rejections: state
            .window_rejections_for_account(account_id)
            .await
            .expect("rejections"),
        account: state
            .load_account(account_id)
            .await
            .expect("account metadata"),
        snapshot: state
            .load_quota_snapshot_for_route_band(account_id, RouteBand::ClaudeMessages.as_str())
            .await
            .expect("snapshot"),
    }
}

async fn seed_protected_state(state: &AsyncSqliteStateStore) -> AccountId {
    let account_id = AccountId::new("claude-status-preservation").expect("account id");
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                "synthetic-claude",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(7),
        )
        .await
        .expect("account metadata");
    state
        .upsert_selector_quota_window(
            &PersistedSelectorQuotaWindow::new(
                account_id.clone(),
                RouteBand::ClaudeMessages.as_str(),
                18_000,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(61)
            .with_observed_unix_seconds(1_000)
            .with_effective(true),
        )
        .await
        .expect("selector window");
    state
        .mark_route_band_quota_exhausted(&account_id, RouteBand::ClaudeMessages.as_str(), 1_100)
        .await
        .expect("route exhaustion state");
    for window_kind in [WindowKind::FiveHour, WindowKind::Weekly] {
        let observation = WindowObservation::new(
            WindowObservationProps::new(account_id.clone(), window_kind, 6_100, 1_000)
                .with_reset_unix_seconds(9_000)
                .with_fresh_until_unix_seconds(2_000),
        )
        .expect("native observation");
        state
            .record_window_observation(&observation, || 1_000)
            .await
            .expect("observation write");
    }
    let rejection = WindowRejection::new(
        WindowRejectionProps::new(account_id.clone(), WindowKind::FiveHour, 1_150)
            .with_reported_reset(9_000),
    );
    state
        .record_window_rejection(&rejection)
        .await
        .expect("rejection barrier");
    state
        .record_refresh_failure_preserving_selector_windows(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_200,
            QuotaRefreshErrorClass::ParseError,
        )
        .await
        .expect("failure metadata");
    account_id
}

#[tokio::test]
async fn status_only_success_records_recovery_without_changing_account_evidence() {
    let directory = QuotaStatusDirectory::new();
    let state = AsyncSqliteStateStore::open(&directory.0.join("state.sqlite"))
        .await
        .expect("state");
    let account_id = seed_protected_state(&state).await;
    let other_account_id = AccountId::new("other-refresh-account").expect("other account id");
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Openai,
                other_account_id.clone(),
                "synthetic-peer",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("other account");
    state
        .record_refresh_failure_preserving_selector_windows(
            &other_account_id,
            RouteBand::Responses.as_str(),
            900,
            QuotaRefreshErrorClass::NetworkError,
        )
        .await
        .expect("other failure metadata");
    let other_status = state
        .quota_refresh_statuses_for_route_band(RouteBand::Responses.as_str())
        .await
        .expect("other refresh status");
    let before = read_preserved_account_state(&state, &account_id).await;
    assert!(!before.selector_windows.is_empty());
    assert!(!before.route_states.is_empty());
    assert_eq!(before.observations.len(), 2);
    assert_eq!(before.rejections.len(), 1);
    assert!(before.snapshot.is_some());

    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_300,
            2_300,
        )
        .await
        .expect("status-only success");
    let status = state
        .quota_refresh_statuses_for_route_band(RouteBand::ClaudeMessages.as_str())
        .await
        .expect("refresh status")
        .pop()
        .expect("recorded status");
    assert_eq!(status.last_success_unix_seconds(), Some(1_300));
    assert_eq!(status.last_attempt_unix_seconds(), Some(1_300));
    assert_eq!(status.last_error_class(), None);
    assert_eq!(status.stale_after_unix_seconds(), Some(2_300));
    assert_eq!(
        read_preserved_account_state(&state, &account_id).await,
        before
    );
    assert_eq!(
        state
            .quota_refresh_statuses_for_route_band(RouteBand::Responses.as_str())
            .await
            .expect("other status"),
        other_status
    );
    state.close().await.expect("close state");
}

#[tokio::test]
async fn combined_success_still_replaces_selectors_and_clears_route_exhaustion() {
    let directory = QuotaStatusDirectory::new();
    let state = AsyncSqliteStateStore::open(&directory.0.join("state.sqlite"))
        .await
        .expect("state");
    let account_id = seed_protected_state(&state).await;
    let before = read_preserved_account_state(&state, &account_id).await;
    let replacement = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        RouteBand::ClaudeMessages.as_str(),
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(45)
    .with_observed_unix_seconds(1_300);
    state
        .record_refresh_success_and_replace_selector_windows(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            &[replacement],
            1_300,
            2_300,
        )
        .await
        .expect("combined refresh success");
    let after = read_preserved_account_state(&state, &account_id).await;
    assert_eq!(after.selector_windows.len(), 1);
    assert_eq!(after.selector_windows[0].limit_window_seconds, 604_800);
    assert_eq!(after.selector_windows[0].remaining_headroom, 45);
    assert!(after.route_states.is_empty());
    assert_eq!(after.observations, before.observations);
    assert_eq!(after.rejections, before.rejections);
    assert_eq!(after.account, before.account);
    let status = state
        .quota_refresh_statuses_for_route_band(RouteBand::ClaudeMessages.as_str())
        .await
        .expect("combined status")
        .pop()
        .expect("recorded status");
    assert_eq!(status.last_success_unix_seconds(), Some(1_300));
    assert_eq!(status.last_attempt_unix_seconds(), Some(1_300));
    assert_eq!(status.last_error_class(), None);
    assert_eq!(status.stale_after_unix_seconds(), Some(2_300));
    state.close().await.expect("close state");
}

#[tokio::test]
async fn status_only_success_rejects_unrepresentable_timestamps_without_writing() {
    let directory = QuotaStatusDirectory::new();
    let state = AsyncSqliteStateStore::open(&directory.0.join("state.sqlite"))
        .await
        .expect("state");
    let account_id = seed_protected_state(&state).await;
    let before = state
        .quota_refresh_statuses_for_route_band(RouteBand::ClaudeMessages.as_str())
        .await
        .expect("status");
    for (last_success, stale_after) in [(u64::MAX, 2_300), (1_300, u64::MAX)] {
        assert!(
            state
                .record_refresh_success_status(
                    &account_id,
                    RouteBand::ClaudeMessages.as_str(),
                    last_success,
                    stale_after
                )
                .await
                .is_err()
        );
    }
    assert_eq!(
        state
            .quota_refresh_statuses_for_route_band(RouteBand::ClaudeMessages.as_str())
            .await
            .expect("status after rejection"),
        before
    );
    state.close().await.expect("close state");
}

#[tokio::test]
async fn status_only_success_on_read_only_store_preserves_existing_failure() {
    let directory = QuotaStatusDirectory::new();
    let database_path = directory.0.join("state.sqlite");
    let state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state");
    let account_id = seed_protected_state(&state).await;
    state.close().await.expect("close writable state");
    let state = AsyncSqliteStateStore::open_read_only(&database_path)
        .await
        .expect("read-only state");
    let before = state
        .quota_refresh_statuses_for_route_band(RouteBand::ClaudeMessages.as_str())
        .await
        .expect("status");
    assert!(
        state
            .record_refresh_success_status(
                &account_id,
                RouteBand::ClaudeMessages.as_str(),
                1_300,
                2_300
            )
            .await
            .is_err()
    );
    assert_eq!(
        state
            .quota_refresh_statuses_for_route_band(RouteBand::ClaudeMessages.as_str())
            .await
            .expect("status after rejected write"),
        before
    );
    state.close().await.expect("close state");
}
