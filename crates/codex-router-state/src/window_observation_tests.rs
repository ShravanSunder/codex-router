use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_router_core::provider::Provider;

use crate::account::AccountRecord;
use crate::account::AccountStatus;
use crate::sqlite::AsyncSqliteStateStore;

use super::WindowObservation;
use super::WindowObservationProps;
use super::WindowRejection;
use super::WindowRejectionProps;

static NEXT_TEMP_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct WindowStateTempDir {
    path: PathBuf,
}

impl WindowStateTempDir {
    fn new() -> Self {
        let unique = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "codex-router-window-state-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("temporary state directory should be created: {error}"));
        Self { path }
    }

    fn database_path(&self) -> PathBuf {
        self.path.join("state.sqlite")
    }
}

impl Drop for WindowStateTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn account_id(label: &str) -> codex_router_core::ids::AccountId {
    codex_router_core::ids::AccountId::new(format!("acct_{label}"))
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"))
}

async fn store_with_claude_account(
    database_path: &std::path::Path,
    label: &str,
) -> (AsyncSqliteStateStore, codex_router_core::ids::AccountId) {
    let state = AsyncSqliteStateStore::open(database_path)
        .await
        .unwrap_or_else(|error| panic!("state database should open: {error}"));
    let account_id = account_id(label);
    state
        .upsert_account(&AccountRecord::new(
            Provider::Claude,
            account_id.clone(),
            label,
            AccountStatus::Enabled,
        ))
        .await
        .unwrap_or_else(|error| panic!("Claude account should persist: {error}"));
    (state, account_id)
}

#[tokio::test]
async fn selector_input_preserves_the_account_provider_from_persistence() {
    let temp_dir = WindowStateTempDir::new();
    let (state, account_id) =
        store_with_claude_account(&temp_dir.database_path(), "provider").await;

    let selector_inputs = state
        .selector_inputs_for_route_band("responses", 1_000)
        .await
        .expect("provider-aware selector inputs should load");
    let selector_input = selector_inputs
        .iter()
        .find(|input| input.account_id() == &account_id)
        .expect("persisted Claude account should appear in selector inputs");

    assert_eq!(selector_input.provider(), Provider::Claude);
}

fn observation(
    account_id: &codex_router_core::ids::AccountId,
    window_kind: codex_router_core::route_profile::WindowKind,
    remaining_basis_points: u32,
    reset_unix_seconds: Option<u64>,
    observation_started_at: u64,
) -> WindowObservation {
    let mut props =
        WindowObservationProps::new(account_id.clone(), window_kind, remaining_basis_points)
            .with_observation_started_at(observation_started_at);
    if let Some(reset_unix_seconds) = reset_unix_seconds {
        props = props.with_reset_unix_seconds(reset_unix_seconds);
    }
    WindowObservation::new(props)
        .unwrap_or_else(|error| panic!("test observation should be valid: {error}"))
}

fn rejection(
    account_id: &codex_router_core::ids::AccountId,
    window_kind: codex_router_core::route_profile::WindowKind,
    rejected_at: u64,
    reported_reset: Option<u64>,
) -> WindowRejection {
    let mut props = WindowRejectionProps::new(account_id.clone(), window_kind, rejected_at);
    if let Some(reported_reset) = reported_reset {
        props = props.with_reported_reset(reported_reset);
    }
    WindowRejection::new(props)
}

#[tokio::test]
async fn poll_started_before_rejection_cannot_clear_it_when_it_finishes_afterward() {
    let temp_dir = WindowStateTempDir::new();
    let database_path = temp_dir.database_path();
    let (state, account_id) = store_with_claude_account(&database_path, "poll_before").await;
    state
        .record_window_rejection(&rejection(
            &account_id,
            codex_router_core::route_profile::WindowKind::FiveHour,
            200,
            Some(500),
        ))
        .await
        .expect("window rejection should persist");

    state
        .record_window_observation(
            &observation(
                &account_id,
                codex_router_core::route_profile::WindowKind::FiveHour,
                1_000,
                Some(600),
                100,
            ),
            300,
            300,
        )
        .await
        .expect("late poll observation should persist");

    assert!(
        state
            .account_window_is_exhausted(&account_id)
            .await
            .unwrap_or(false)
    );
}

#[tokio::test]
async fn poll_that_starts_after_rejection_but_lands_stale_cannot_clear_it() {
    let temp_dir = WindowStateTempDir::new();
    let database_path = temp_dir.database_path();
    let (state, account_id) = store_with_claude_account(&database_path, "stale_poll").await;
    state
        .record_window_rejection(&rejection(
            &account_id,
            codex_router_core::route_profile::WindowKind::Weekly,
            200,
            Some(900),
        ))
        .await
        .expect("window rejection should persist");

    state
        .record_window_observation(
            &observation(
                &account_id,
                codex_router_core::route_profile::WindowKind::Weekly,
                1_000,
                Some(1_000),
                250,
            ),
            1_000,
            300,
        )
        .await
        .expect("stale poll observation should persist");

    assert!(
        state
            .account_window_is_exhausted(&account_id)
            .await
            .unwrap_or(false)
    );
}

#[tokio::test]
async fn per_window_upsert_keeps_the_observation_with_the_newer_start_time() {
    let temp_dir = WindowStateTempDir::new();
    let database_path = temp_dir.database_path();
    let (state, account_id) = store_with_claude_account(&database_path, "ordered_poll").await;

    assert!(
        state
            .record_window_observation(
                &observation(
                    &account_id,
                    codex_router_core::route_profile::WindowKind::Weekly,
                    8_000,
                    Some(900),
                    300,
                ),
                320,
                300,
            )
            .await
            .expect("new observation should be recorded")
    );
    assert!(
        !state
            .record_window_observation(
                &observation(
                    &account_id,
                    codex_router_core::route_profile::WindowKind::Weekly,
                    5_000,
                    Some(1_000),
                    250,
                ),
                330,
                300,
            )
            .await
            .expect("older observation should not replace the newest row")
    );

    let observations = state
        .window_observations_for_account(&account_id)
        .await
        .expect("newest observation should load");
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].remaining_basis_points(), 8_000);
    assert_eq!(observations[0].observation_started_at(), 300);
}

#[tokio::test]
async fn concurrent_rejections_for_two_windows_remain_independently_exhausted() {
    let temp_dir = WindowStateTempDir::new();
    let database_path = temp_dir.database_path();
    let (state, account_id) = store_with_claude_account(&database_path, "two_windows").await;
    let five_hour = rejection(
        &account_id,
        codex_router_core::route_profile::WindowKind::FiveHour,
        200,
        Some(500),
    );
    let weekly = rejection(
        &account_id,
        codex_router_core::route_profile::WindowKind::Weekly,
        201,
        Some(900),
    );

    let (five_hour_result, weekly_result) = tokio::join!(
        state.record_window_rejection(&five_hour),
        state.record_window_rejection(&weekly),
    );
    five_hour_result.expect("five-hour rejection should persist");
    weekly_result.expect("weekly rejection should persist");

    let rejections = state
        .window_rejections_for_account(&account_id)
        .await
        .expect("both rejection rows should load");
    assert_eq!(rejections.len(), 2);
    assert!(
        state
            .account_window_is_exhausted(&account_id)
            .await
            .unwrap_or(false)
    );
}

#[tokio::test]
async fn duplicate_rejections_keep_the_latest_timestamp_and_its_reset() {
    let temp_dir = WindowStateTempDir::new();
    let database_path = temp_dir.database_path();
    let (state, account_id) = store_with_claude_account(&database_path, "duplicate").await;

    state
        .record_window_rejection(&rejection(
            &account_id,
            codex_router_core::route_profile::WindowKind::FiveHour,
            300,
            Some(400),
        ))
        .await
        .expect("newest rejection should persist");
    state
        .record_window_rejection(&rejection(
            &account_id,
            codex_router_core::route_profile::WindowKind::FiveHour,
            250,
            Some(600),
        ))
        .await
        .expect("older duplicate should be idempotent");

    let rejections = state
        .window_rejections_for_account(&account_id)
        .await
        .expect("rejection should load");
    assert_eq!(rejections.len(), 1);
    assert_eq!(rejections[0].rejected_at(), 300);
    assert_eq!(rejections[0].reported_reset(), Some(400));
}

#[tokio::test]
async fn partial_observation_updates_only_its_window_and_fresh_headroom_clears_that_window() {
    let temp_dir = WindowStateTempDir::new();
    let database_path = temp_dir.database_path();
    let (state, account_id) = store_with_claude_account(&database_path, "partial").await;
    for (window_kind, start) in [
        (codex_router_core::route_profile::WindowKind::FiveHour, 100),
        (codex_router_core::route_profile::WindowKind::Weekly, 110),
    ] {
        state
            .record_window_observation(
                &observation(&account_id, window_kind, 5_000, Some(900), start),
                120,
                300,
            )
            .await
            .expect("full poll window should persist");
    }
    state
        .record_window_rejection(&rejection(
            &account_id,
            codex_router_core::route_profile::WindowKind::FiveHour,
            200,
            Some(600),
        ))
        .await
        .expect("five-hour rejection should persist");

    state
        .record_window_observation(
            &observation(
                &account_id,
                codex_router_core::route_profile::WindowKind::FiveHour,
                2_500,
                Some(1_000),
                250,
            ),
            260,
            300,
        )
        .await
        .expect("fresh passive window should persist");

    let observations = state
        .window_observations_for_account(&account_id)
        .await
        .expect("partial observations should load");
    assert_eq!(observations.len(), 2);
    let weekly = observations
        .iter()
        .find(|item| item.window_kind() == codex_router_core::route_profile::WindowKind::Weekly)
        .expect("weekly window from the full poll should remain");
    assert_eq!(weekly.remaining_basis_points(), 5_000);
    let five_hour = observations
        .iter()
        .find(|item| item.window_kind() == codex_router_core::route_profile::WindowKind::FiveHour)
        .expect("passive five-hour window should be stored");
    assert_eq!(five_hour.remaining_basis_points(), 2_500);
    assert!(
        !state
            .account_window_is_exhausted(&account_id)
            .await
            .unwrap_or(true)
    );
}

#[tokio::test]
async fn zero_headroom_observation_does_not_clear_a_rejection() {
    let temp_dir = WindowStateTempDir::new();
    let database_path = temp_dir.database_path();
    let (state, account_id) = store_with_claude_account(&database_path, "zero_headroom").await;
    state
        .record_window_rejection(&rejection(
            &account_id,
            codex_router_core::route_profile::WindowKind::FiveHour,
            200,
            Some(500),
        ))
        .await
        .expect("window rejection should persist");

    state
        .record_window_observation(
            &observation(
                &account_id,
                codex_router_core::route_profile::WindowKind::FiveHour,
                0,
                Some(600),
                250,
            ),
            260,
            300,
        )
        .await
        .expect("zero-headroom observation should persist");

    assert!(
        state
            .account_window_is_exhausted(&account_id)
            .await
            .unwrap_or(false)
    );
}

#[tokio::test]
async fn observations_and_rejections_persist_across_state_store_restart() {
    let temp_dir = WindowStateTempDir::new();
    let database_path = temp_dir.database_path();
    let (state, account_id) = store_with_claude_account(&database_path, "restart").await;
    state
        .record_window_observation(
            &observation(
                &account_id,
                codex_router_core::route_profile::WindowKind::Weekly,
                5_000,
                Some(900),
                100,
            ),
            120,
            300,
        )
        .await
        .expect("observation should persist");
    state
        .record_window_rejection(&rejection(
            &account_id,
            codex_router_core::route_profile::WindowKind::Weekly,
            200,
            Some(900),
        ))
        .await
        .expect("rejection should persist");
    state
        .close()
        .await
        .expect("state store should close before restart");

    let reopened = AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("database should reopen: {error}"));
    assert!(
        reopened
            .account_window_is_exhausted(&account_id)
            .await
            .unwrap_or(false)
    );
    assert_eq!(
        reopened
            .window_observations_for_account(&account_id)
            .await
            .map(|observations| observations.len()),
        Ok(1)
    );
    assert_eq!(
        reopened
            .window_rejections_for_account(&account_id)
            .await
            .map(|rejections| rejections.len()),
        Ok(1)
    );
    reopened
        .close()
        .await
        .expect("reopened state store should close");
}

#[tokio::test]
async fn selector_inputs_preserve_the_provider_from_account_storage() {
    let temp_dir = WindowStateTempDir::new();
    let database_path = temp_dir.database_path();
    let (state, account_id) =
        store_with_claude_account(&database_path, "provider_projection").await;

    let selector_inputs = state
        .selector_inputs_for_route_band("claude_messages", 1_000)
        .await
        .expect("stored Claude account should project to selector input");

    assert_eq!(selector_inputs.len(), 1);
    assert_eq!(selector_inputs[0].account_id(), &account_id);
    assert_eq!(selector_inputs[0].provider(), Provider::Claude);
}
