use super::*;
use http_body_util::BodyExt;
use sqlx::Connection;

#[tokio::test]
async fn degraded_queue_health_emits_static_rejection_and_keeps_503_body() {
    let (_temporary_directory, store) = open_local_migrated_store().await;
    let queue_health = super::RouteBandQueueHealth::default();
    super::mark_route_band_queue_degraded(
        &queue_health,
        RouteBand::Responses,
        super::RouteBandQueueDegradedReason::DbWriteQueueFull,
        1_000,
    )
    .unwrap_or_else(|error| panic!("queue health should degrade: {error}"));
    let selector = super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        &store,
        super::AsyncAccountSelectorRuntimeState::new(
            super::RouteBandWeightedSelectors::default(),
            super::RouteBandAccountHolds::default(),
            super::RouteBandReservationBooks::default(),
            super::RouteBandRuntimeExhaustions::default(),
            queue_health,
        ),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        Arc::new(|| 1_000),
    );
    let request = crate::http_sse::HttpProxyRequest::new(
        crate::routes::Method::Post,
        "/v1/responses?query_private_marker",
    );

    let (captured_logs, result) = crate::test_log_capture::capture_log_output_async(async {
        selector
            .select_upstream_account(&request, TokenGeneration::new(1), None)
            .await
    })
    .await;

    assert!(
        captured_logs.contains("codex_router.selection_rejected"),
        "queue-health rejection should emit the independent selection diagnostic: {captured_logs}"
    );
    assert!(captured_logs.contains("selection.stage=\"queue_health\""));
    assert!(captured_logs.contains("error.class=\"sqlite\""));
    assert!(captured_logs.contains("route.band=\"responses\""));
    assert!(!captured_logs.contains("query_private_marker"));
    let error = result.expect_err("degraded queue health must continue to fail closed");
    assert!(matches!(
        &error,
        crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::StateUnavailable,
        }
    ));

    let response = crate::server::http_error_response(error);
    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    let response_body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("selection response body should collect: {error}"))
        .to_bytes();
    assert_eq!(
        response_body.as_ref(),
        br#"{"type":"error","status":503,"error":{"type":"codex_router_quota_state_unavailable","code":"codex_router_quota_state_unavailable","message":"codex-router cannot safely rotate accounts because quota state is unavailable."}}"#
    );
}

#[tokio::test]
async fn projection_failure_keeps_its_fixed_sqlite_error_class() {
    let (temporary_directory, store) = open_local_migrated_store().await;
    let database_path = temporary_directory.path().join("selection.sqlite");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database_path)
        .create_if_missing(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap_or_else(|error| panic!("local fixture connection should open: {error}"));
    sqlx::query("DROP TABLE account_routing_policies")
        .execute(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("local projection table should be dropped: {error}"));
    connection
        .close()
        .await
        .unwrap_or_else(|error| panic!("local fixture connection should close: {error}"));
    let selector = selector_with_runtime_state(&store, default_runtime_state());
    let request = responses_request();

    let (captured_logs, result) = crate::test_log_capture::capture_log_output_async(async {
        selector
            .select_upstream_account(&request, TokenGeneration::new(1), None)
            .await
    })
    .await;

    assert!(captured_logs.contains("codex_router.selection_rejected"));
    assert!(captured_logs.contains("selection.stage=\"persisted_projection\""));
    assert!(captured_logs.contains("error.class=\"sqlite\""));
    assert!(matches!(
        result,
        Err(crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::StateUnavailable,
        })
    ));
}

#[tokio::test]
async fn corrupt_sqlite_account_emits_typed_class_without_account_payload() {
    let (temporary_directory, store) = open_local_migrated_store().await;
    let account_id = seed_eligible_account(&store, "acct_private_marker").await;
    let database_path = temporary_directory.path().join("selection.sqlite");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database_path)
        .create_if_missing(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap_or_else(|error| panic!("local fixture connection should open: {error}"));
    sqlx::query("UPDATE accounts SET label = ?, status = ? WHERE account_id = ?")
        .bind("label_private_marker")
        .bind("status_private_marker")
        .bind(account_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("local test account should become corrupt: {error}"));
    connection
        .close()
        .await
        .unwrap_or_else(|error| panic!("local fixture connection should close: {error}"));
    let selector = selector_with_runtime_state(&store, default_runtime_state());

    let (captured_logs, result) = crate::test_log_capture::capture_log_output_async(async {
        selector
            .select_upstream_account(&responses_request(), TokenGeneration::new(1), None)
            .await
    })
    .await;

    assert!(captured_logs.contains("selection.stage=\"persisted_projection\""));
    assert!(captured_logs.contains("error.class=\"corrupt_account\""));
    assert!(!captured_logs.contains("acct_private_marker"));
    assert!(!captured_logs.contains("label_private_marker"));
    assert!(!captured_logs.contains("status_private_marker"));
    assert!(matches!(
        result,
        Err(crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::StateUnavailable,
        })
    ));
}

#[tokio::test]
async fn runtime_quarantine_lock_failure_is_separate_from_projection_failure() {
    let (_temporary_directory, store) = open_local_migrated_store().await;
    let runtime_exhaustions = super::RouteBandRuntimeExhaustions::default();
    let poisoned_runtime_exhaustions = Arc::clone(&runtime_exhaustions);
    let poison_result = std::thread::spawn(move || {
        let _guard = poisoned_runtime_exhaustions.lock().unwrap_or_else(|error| {
            panic!("runtime exhaustion lock should start healthy: {error}")
        });
        panic!("poison only the test runtime quarantine lock");
    })
    .join();
    assert!(poison_result.is_err());
    let runtime_state = super::AsyncAccountSelectorRuntimeState::new(
        super::RouteBandWeightedSelectors::default(),
        super::RouteBandAccountHolds::default(),
        super::RouteBandReservationBooks::default(),
        runtime_exhaustions,
        super::RouteBandQueueHealth::default(),
    );
    let selector = selector_with_runtime_state(&store, runtime_state);
    let request = responses_request();

    let (captured_logs, result) = crate::test_log_capture::capture_log_output_async(async {
        selector
            .select_upstream_account(&request, TokenGeneration::new(1), None)
            .await
    })
    .await;

    assert!(captured_logs.contains("codex_router.selection_rejected"));
    assert!(captured_logs.contains("selection.stage=\"runtime_quarantine\""));
    assert!(captured_logs.contains("error.class=\"lock_poisoned\""));
    assert!(matches!(
        result,
        Err(crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::SelectorStateUnavailable,
        })
    ));
}

#[tokio::test]
async fn missing_previous_response_owner_has_a_fixed_affinity_class() {
    let (_temporary_directory, store) = open_local_migrated_store().await;
    let selector = selector_with_runtime_state(&store, default_runtime_state());
    let request = responses_request()
        .with_body(br#"{"previous_response_id":"payload_private_marker"}"#.to_vec());
    let affinity_secret = codex_router_core::affinity::RouterAffinityHashSecret::new(
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should be valid: {error}"));

    let (captured_logs, result) = crate::test_log_capture::capture_log_output_async(async {
        selector
            .select_upstream_account(&request, TokenGeneration::new(1), Some(&affinity_secret))
            .await
    })
    .await;

    assert!(captured_logs.contains("codex_router.selection_rejected"));
    assert!(captured_logs.contains("selection.stage=\"previous_response_affinity\""));
    assert!(captured_logs.contains("error.class=\"affinity_owner_missing\""));
    assert!(!captured_logs.contains("payload_private_marker"));
    assert!(matches!(
        result,
        Err(crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::AffinityOwnerMissing,
        })
    ));
}

#[tokio::test]
async fn healthy_sqlite_selection_keeps_the_selected_account_without_rejection() {
    let (_temporary_directory, store) = open_local_migrated_store().await;
    let account_id = seed_eligible_account(&store, "acct_selection_diagnostics").await;
    let selector = selector_with_runtime_state(&store, default_runtime_state());
    let request = responses_request();

    let (captured_logs, result) = crate::test_log_capture::capture_log_output_async(async {
        selector
            .select_upstream_account(&request, TokenGeneration::new(1), None)
            .await
    })
    .await;

    let selected = result.unwrap_or_else(|error| panic!("healthy selector should select: {error}"));
    assert_eq!(selected.account_id(), &account_id);
    assert!(!captured_logs.contains("codex_router.selection_rejected"));
}

#[tokio::test]
async fn weighted_selector_lock_failure_is_named_as_a_selector_failure() {
    let (_temporary_directory, store) = open_local_migrated_store().await;
    let _account_id = seed_eligible_account(&store, "acct_weighted_selector_lock").await;
    let weighted_selectors = super::RouteBandWeightedSelectors::default();
    poison_mutex(Arc::clone(&weighted_selectors));
    let selector = selector_with_runtime_state(
        &store,
        super::AsyncAccountSelectorRuntimeState::new(
            weighted_selectors,
            super::RouteBandAccountHolds::default(),
            super::RouteBandReservationBooks::default(),
            super::RouteBandRuntimeExhaustions::default(),
            super::RouteBandQueueHealth::default(),
        ),
    );

    let (captured_logs, result) = crate::test_log_capture::capture_log_output_async(async {
        selector
            .select_upstream_account(&responses_request(), TokenGeneration::new(1), None)
            .await
    })
    .await;

    assert!(captured_logs.contains("selection.stage=\"weighted_selector\""));
    assert!(captured_logs.contains("error.class=\"lock_poisoned\""));
    assert!(matches!(
        result,
        Err(crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::SelectorStateUnavailable,
        })
    ));
}

#[tokio::test]
async fn account_hold_lock_failure_is_separate_from_weighted_selector_failure() {
    let (_temporary_directory, store) = open_local_migrated_store().await;
    let _account_id = seed_eligible_account(&store, "acct_account_hold_lock").await;
    let account_holds = super::RouteBandAccountHolds::default();
    poison_mutex(Arc::clone(&account_holds));
    let selector = selector_with_runtime_state(
        &store,
        super::AsyncAccountSelectorRuntimeState::new(
            super::RouteBandWeightedSelectors::default(),
            account_holds,
            super::RouteBandReservationBooks::default(),
            super::RouteBandRuntimeExhaustions::default(),
            super::RouteBandQueueHealth::default(),
        ),
    );

    let (captured_logs, result) = crate::test_log_capture::capture_log_output_async(async {
        selector
            .select_upstream_account(&responses_request(), TokenGeneration::new(1), None)
            .await
    })
    .await;

    assert!(captured_logs.contains("selection.stage=\"account_hold\""));
    assert!(captured_logs.contains("error.class=\"lock_poisoned\""));
    assert!(matches!(
        result,
        Err(crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::SelectorStateUnavailable,
        })
    ));
}

#[tokio::test]
async fn empty_sqlite_selection_emits_no_eligible_assessment_class() {
    let (_temporary_directory, store) = open_local_migrated_store().await;
    let selector = selector_with_runtime_state(&store, default_runtime_state());

    let (captured_logs, result) = crate::test_log_capture::capture_log_output_async(async {
        selector
            .select_upstream_account(&responses_request(), TokenGeneration::new(1), None)
            .await
    })
    .await;

    assert!(captured_logs.contains("selection.stage=\"assessment\""));
    assert!(captured_logs.contains("error.class=\"no_eligible_accounts\""));
    assert!(matches!(
        result,
        Err(crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::NoEligibleAccounts,
        })
    ));
}

async fn open_local_migrated_store() -> (
    tempfile::TempDir,
    codex_router_state::sqlite::AsyncSqliteStateStore,
) {
    let temporary_directory = tempfile::tempdir()
        .unwrap_or_else(|error| panic!("temporary test directory should open: {error}"));
    let database_path = temporary_directory.path().join("selection.sqlite");
    let store = codex_router_state::sqlite::AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("local migrated selector state should open: {error}"));
    (temporary_directory, store)
}

async fn seed_eligible_account(
    store: &codex_router_state::sqlite::AsyncSqliteStateStore,
    account_id_value: &str,
) -> codex_router_core::ids::AccountId {
    let account_id = codex_router_core::ids::AccountId::new(account_id_value)
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"));
    let account = codex_router_state::account::AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "selection diagnostics",
        codex_router_state::account::AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    store
        .upsert_account(&account)
        .await
        .unwrap_or_else(|error| panic!("test account should persist: {error}"));
    for (window_seconds, effective) in [
        (
            codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
            true,
        ),
        (
            codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
            false,
        ),
    ] {
        let window = codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            window_seconds,
            codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(90)
        .with_effective(effective)
        .with_observed_unix_seconds(1_000)
        .with_reset_unix_seconds(1_000 + window_seconds);
        store
            .upsert_selector_quota_window(&window)
            .await
            .unwrap_or_else(|error| panic!("test selector window should persist: {error}"));
    }
    account_id
}

fn poison_mutex<TSend: Send + 'static>(mutex: Arc<Mutex<TSend>>) {
    let poison_result = std::thread::spawn(move || {
        let _guard = mutex
            .lock()
            .unwrap_or_else(|error| panic!("test mutex should start healthy: {error}"));
        panic!("poison only the test selector mutex");
    })
    .join();
    assert!(poison_result.is_err());
}

fn default_runtime_state() -> super::AsyncAccountSelectorRuntimeState {
    super::AsyncAccountSelectorRuntimeState::new(
        super::RouteBandWeightedSelectors::default(),
        super::RouteBandAccountHolds::default(),
        super::RouteBandReservationBooks::default(),
        super::RouteBandRuntimeExhaustions::default(),
        super::RouteBandQueueHealth::default(),
    )
}

fn selector_with_runtime_state<'a>(
    store: &'a codex_router_state::sqlite::AsyncSqliteStateStore,
    runtime_state: super::AsyncAccountSelectorRuntimeState,
) -> super::AsyncRepositoryBackedAccountSelector<
    'a,
    codex_router_state::sqlite::AsyncSqliteStateStore,
> {
    super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        store,
        runtime_state,
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        Arc::new(|| 1_000),
    )
}

fn responses_request() -> crate::http_sse::HttpProxyRequest {
    crate::http_sse::HttpProxyRequest::new(crate::routes::Method::Post, "/v1/responses")
}
