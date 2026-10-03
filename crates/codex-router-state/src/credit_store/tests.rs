use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditBalance;
use codex_router_core::credit_usage::CreditProviderLimitReason;
use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_core::credit_usage::CreditSpendControl;
use codex_router_core::credit_usage::CreditUsagePolicy;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use sqlx::Connection as _;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteConnection;
use sqlx::sqlite::SqliteJournalMode;

use crate::account::AccountRecord;
use crate::account::AccountStatus;
use crate::credit_store::AsyncCreditUsagePolicyMutationStore;
use crate::credit_store::ResponsesRefreshSuccessCommit;
use crate::quota_snapshot::PersistedQuotaHistoryObservation;
use crate::quota_snapshot::PersistedQuotaSnapshot;
use crate::quota_snapshot::PersistedSelectorQuotaWindow;
use crate::quota_snapshot::QuotaHistoryRefreshOutcome;
use crate::quota_snapshot::QuotaRefreshErrorClass;
use crate::quota_snapshot::QuotaSnapshotSource;
use crate::quota_snapshot::SelectorQuotaWindowStatus;
use crate::sqlite::AsyncSqliteStateStore;
use crate::sqlite::StateStoreError;

#[path = "observation_read_rejection_tests.rs"]
mod observation_read_rejection_tests;
#[path = "suspect_exhausted_credit_suppression_tests.rs"]
mod suspect_exhausted_credit_suppression_tests;

static NEXT_TEMP_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct CreditStoreTempDir {
    path: PathBuf,
}

impl CreditStoreTempDir {
    fn new() -> Self {
        let unique = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "codex-router-credit-state-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("temporary state directory should exist: {error}"));
        Self { path }
    }

    fn database_path(&self) -> PathBuf {
        self.path.join("state.sqlite")
    }
}

impl Drop for CreditStoreTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn account_id(label: &str) -> AccountId {
    AccountId::new(format!("acct_{label}"))
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"))
}

async fn openai_account(
    database_path: &std::path::Path,
    label: &str,
    generation: u64,
) -> (AsyncSqliteStateStore, AccountId) {
    let state = AsyncSqliteStateStore::open(database_path)
        .await
        .unwrap_or_else(|error| panic!("state database should open: {error}"));
    let account_id = account_id(label);
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Openai,
                account_id.clone(),
                label,
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(generation),
        )
        .await
        .unwrap_or_else(|error| panic!("OpenAI account should persist: {error}"));
    (state, account_id)
}

fn provider_observation(
    balance: &str,
    spend_control: CreditSpendControl,
    limit_reason: Option<CreditProviderLimitReason>,
) -> CreditProviderObservation {
    CreditProviderObservation::new(
        CreditAvailability::Available {
            balance: Some(
                CreditBalance::new(balance)
                    .unwrap_or_else(|error| panic!("test balance should validate: {error}")),
            ),
        },
        spend_control,
        limit_reason,
    )
}

fn selector_window(
    account_id: &AccountId,
    remaining_headroom: u32,
    observed_unix_seconds: u64,
) -> PersistedSelectorQuotaWindow {
    let status = if remaining_headroom == 0 {
        SelectorQuotaWindowStatus::Ineligible
    } else {
        SelectorQuotaWindowStatus::Eligible
    };
    PersistedSelectorQuotaWindow::new(account_id.clone(), "responses", 604_800, status)
        .with_remaining_headroom(remaining_headroom)
        .with_effective(true)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_reset_unix_seconds(observed_unix_seconds + 604_800)
}

fn successful_history_observation(
    account_id: &AccountId,
    remaining_headroom: u32,
    observed_unix_seconds: u64,
) -> PersistedQuotaHistoryObservation {
    let status = if remaining_headroom == 0 {
        SelectorQuotaWindowStatus::Ineligible
    } else {
        SelectorQuotaWindowStatus::Eligible
    };
    PersistedQuotaHistoryObservation::new(
        account_id.clone(),
        account_id.as_str(),
        "responses",
        604_800,
        observed_unix_seconds,
        remaining_headroom,
    )
    .with_reset_unix_seconds(observed_unix_seconds + 604_800)
    .with_effective(true)
    .with_window_status(status)
    .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
    .with_refresh_outcome(QuotaHistoryRefreshOutcome::Success)
}

fn quota_snapshot(
    account_id: &AccountId,
    remaining_headroom: u32,
    observed_unix_seconds: u64,
) -> PersistedQuotaSnapshot {
    PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_route_band("responses", remaining_headroom)
        .with_reset_unix_seconds(observed_unix_seconds + 604_800)
        .with_stale_penalty(false)
}

fn failed_history_observations(
    account_id: &AccountId,
    observed_unix_seconds: u64,
    error_class: QuotaRefreshErrorClass,
) -> [PersistedQuotaHistoryObservation; 2] {
    [18_000, 604_800].map(|limit_window_seconds| {
        PersistedQuotaHistoryObservation::new(
            account_id.clone(),
            account_id.as_str(),
            "responses",
            limit_window_seconds,
            observed_unix_seconds,
            0,
        )
        .with_window_status(SelectorQuotaWindowStatus::Unknown)
        .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
        .with_refresh_outcome(QuotaHistoryRefreshOutcome::Failure { error_class })
    })
}

#[tokio::test]
async fn account_credit_policy_defaults_disallow_and_survives_reopen() {
    let temporary_directory = CreditStoreTempDir::new();
    let database_path = temporary_directory.database_path();
    let (state, account_id) = openai_account(&database_path, "policy", 1).await;

    assert_eq!(
        state
            .load_account_credit_usage_policy(&account_id)
            .await
            .expect("missing policy should default safely"),
        CreditUsagePolicy::Disallow
    );
    state
        .save_account_credit_usage_policy(&account_id, CreditUsagePolicy::Allow)
        .await
        .expect("explicit Allow should persist");
    state.close().await.expect("state should close");

    let reopened = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should reopen");
    assert_eq!(
        reopened
            .load_account_credit_usage_policy(&account_id)
            .await
            .expect("saved policy should read back"),
        CreditUsagePolicy::Allow
    );
}

#[tokio::test]
async fn credit_policy_mutation_reads_back_and_rejects_replaced_generation() {
    let temporary_directory = CreditStoreTempDir::new();
    let database_path = temporary_directory.database_path();
    let (state, account_id) = openai_account(&database_path, "policy_mutation", 1).await;
    state
        .close()
        .await
        .expect("state should close before mutation store opens");

    let mutation = AsyncCreditUsagePolicyMutationStore::open(&database_path)
        .await
        .expect("current native state should support policy mutation");
    assert_eq!(
        mutation
            .save_account_credit_usage_policy(&account_id, Some(1), CreditUsagePolicy::Allow)
            .await
            .expect("matching generation should save and read back"),
        CreditUsagePolicy::Allow
    );
    assert_eq!(
        mutation
            .save_account_credit_usage_policy(&account_id, Some(0), CreditUsagePolicy::Disallow)
            .await
            .expect_err("stale generation must not overwrite saved policy"),
        StateStoreError::CreditUsagePolicyTargetChanged
    );
    mutation.close().await;

    let reopened = AsyncSqliteStateStore::open_read_only(&database_path)
        .await
        .expect("saved policy should reopen read-only");
    assert_eq!(
        reopened
            .load_account_credit_usage_policy(&account_id)
            .await
            .expect("saved policy should read"),
        CreditUsagePolicy::Allow
    );
}

#[tokio::test]
async fn credit_policy_mutation_never_migrates_a_missing_native_table() {
    let temporary_directory = CreditStoreTempDir::new();
    let database_path = temporary_directory.database_path();
    let (state, _) = openai_account(&database_path, "policy_schema", 1).await;
    state
        .close()
        .await
        .expect("state should close before schema fixture edit");

    let options = SqliteConnectOptions::new()
        .filename(&database_path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("test database should open");
    sqlx::query("DROP TABLE account_credit_policies")
        .execute(&mut connection)
        .await
        .expect("test should remove only the credit policy table");
    connection
        .close()
        .await
        .expect("test database should close after fixture edit");

    assert_eq!(
        AsyncCreditUsagePolicyMutationStore::open(&database_path)
            .await
            .expect_err("interactive save must not run migrations"),
        StateStoreError::CreditUsagePolicySchemaUpgradeRequired
    );
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("test database should reopen");
    let table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'account_credit_policies'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("table existence should read");
    assert_eq!(table_count, 0);
    connection
        .close()
        .await
        .expect("test database should close after fixture read");
}

#[tokio::test]
async fn latest_started_same_second_refresh_wins_atomic_quota_credit_pair() {
    let temporary_directory = CreditStoreTempDir::new();
    let (state, account_id) =
        openai_account(&temporary_directory.database_path(), "ordering", 1).await;
    let older_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("first attempt should allocate");
    let newer_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("newer attempt should allocate before provider IO");

    assert_eq!(older_attempt.sequence(), 1);
    assert_eq!(newer_attempt.sequence(), 2);
    assert!(
        state
            .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
                attempt: &newer_attempt,
                selector_windows: &[selector_window(&account_id, 64, 100)],
                observed_unix_seconds: 100,
                stale_after_unix_seconds: 460,
                provider_observation: &provider_observation(
                    "4.1250",
                    CreditSpendControl::Clear,
                    Some(CreditProviderLimitReason::RateLimitReached),
                ),
                history_observations: &[successful_history_observation(&account_id, 64, 100)],
                snapshot: &quota_snapshot(&account_id, 64, 100),
            },)
            .await
            .expect("latest started attempt should commit")
    );
    assert!(
        !state
            .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
                attempt: &older_attempt,
                selector_windows: &[selector_window(&account_id, 0, 100)],
                observed_unix_seconds: 100,
                stale_after_unix_seconds: 460,
                provider_observation: &CreditProviderObservation::missing(),
                history_observations: &[successful_history_observation(&account_id, 0, 100)],
                snapshot: &quota_snapshot(&account_id, 0, 100),
            },)
            .await
            .expect("superseded attempt should be rejected without error")
    );

    let observation = state
        .load_account_credit_observation(&account_id)
        .await
        .expect("credit observation should read")
        .expect("latest successful provider response should persist");
    assert_eq!(observation.latest_started_attempt(), 2);
    assert_eq!(observation.committed_attempt(), Some(2));
    assert_eq!(observation.observed_unix_seconds(), Some(100));
    assert!(observation.authorizes_credit_usage(Some(1), 101));
    assert_eq!(
        observation.provider_observation().availability(),
        &CreditAvailability::Available {
            balance: Some(CreditBalance::new("4.1250").expect("test balance")),
        }
    );

    let selector_inputs = state
        .selector_inputs_for_route_band("responses", 101)
        .await
        .expect("paired selector inputs should read");
    let selector_input = selector_inputs
        .iter()
        .find(|input| input.account_id() == &account_id)
        .expect("account should remain in selector inputs");
    assert_eq!(selector_input.windows()[0].remaining_headroom(), 64);
    let history = state
        .quota_history_observations_for_window(&account_id, "responses", 604_800, 0, 100)
        .await
        .expect("committed history should read");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].remaining_headroom(), 64);
    let snapshot = state
        .load_quota_snapshot_for_route_band(&account_id, "responses")
        .await
        .expect("committed scalar snapshot should read")
        .expect("newest commit should store scalar snapshot");
    assert_eq!(snapshot.remaining_headroom(), 64);
    assert_eq!(snapshot.observed_unix_seconds(), 100);
}

#[tokio::test]
async fn stale_responses_failure_cannot_replace_success_status_history_or_snapshot() {
    let temporary_directory = CreditStoreTempDir::new();
    let (state, account_id) =
        openai_account(&temporary_directory.database_path(), "stale_failure", 1).await;
    let older_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("older attempt should allocate");
    let latest_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("latest attempt should allocate before provider IO");
    let latest_windows = [selector_window(&account_id, 72, 200)];
    let latest_history = [successful_history_observation(&account_id, 72, 200)];
    let latest_snapshot = quota_snapshot(&account_id, 72, 200);

    assert!(
        state
            .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
                attempt: &latest_attempt,
                selector_windows: &latest_windows,
                observed_unix_seconds: 200,
                stale_after_unix_seconds: 560,
                provider_observation: &provider_observation(
                    "3.25",
                    CreditSpendControl::Clear,
                    None,
                ),
                history_observations: &latest_history,
                snapshot: &latest_snapshot,
            },)
            .await
            .expect("latest attempt should commit")
    );
    assert!(
        !state
            .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
                attempt: &older_attempt,
                selector_windows: &[selector_window(&account_id, 0, 150)],
                observed_unix_seconds: 150,
                stale_after_unix_seconds: 510,
                provider_observation: &CreditProviderObservation::missing(),
                history_observations: &[successful_history_observation(&account_id, 0, 150)],
                snapshot: &quota_snapshot(&account_id, 0, 150),
            },)
            .await
            .expect("superseded success should be rejected")
    );
    assert!(
        !state
            .record_responses_refresh_failure(
                &older_attempt,
                150,
                QuotaRefreshErrorClass::NetworkError,
                &failed_history_observations(
                    &account_id,
                    150,
                    QuotaRefreshErrorClass::NetworkError,
                ),
            )
            .await
            .expect("superseded failure should be rejected")
    );

    let statuses = state
        .quota_refresh_statuses_for_route_band("responses")
        .await
        .expect("refresh status should read");
    let status = statuses
        .iter()
        .find(|status| status.account_id() == &account_id)
        .expect("latest success should retain refresh status");
    assert_eq!(status.last_success_unix_seconds(), Some(200));
    assert_eq!(status.last_attempt_unix_seconds(), Some(200));
    assert_eq!(status.last_error_class(), None);

    let history = state
        .quota_history_observations_for_window(&account_id, "responses", 604_800, 0, 250)
        .await
        .expect("history should read");
    assert_eq!(
        history.len(),
        1,
        "superseded failure must add no rollup point"
    );
    assert_eq!(
        history[0].refresh_outcome(),
        QuotaHistoryRefreshOutcome::Success
    );
    assert_eq!(history[0].remaining_headroom(), 72);
    let snapshot = state
        .load_quota_snapshot_for_route_band(&account_id, "responses")
        .await
        .expect("snapshot should read")
        .expect("latest success snapshot should remain");
    assert_eq!(snapshot.remaining_headroom(), 72);
    assert_eq!(snapshot.observed_unix_seconds(), 200);

    let newer_failure_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("newer failed attempt should allocate");
    assert!(
        state
            .record_responses_refresh_failure(
                &newer_failure_attempt,
                250,
                QuotaRefreshErrorClass::NetworkError,
                &failed_history_observations(
                    &account_id,
                    250,
                    QuotaRefreshErrorClass::NetworkError,
                ),
            )
            .await
            .expect("latest failure status should commit")
    );
    let observation = state
        .load_account_credit_observation(&account_id)
        .await
        .expect("cached credit observation should read")
        .expect("latest successful facts should remain cached");
    assert_eq!(observation.latest_started_attempt(), 3);
    assert_eq!(observation.committed_attempt(), Some(2));
    assert_eq!(observation.observed_unix_seconds(), Some(200));
    assert!(!observation.authorizes_credit_usage(Some(1), 251));
    let statuses = state
        .quota_refresh_statuses_for_route_band("responses")
        .await
        .expect("latest failure status should read");
    let status = statuses
        .iter()
        .find(|status| status.account_id() == &account_id)
        .expect("latest failure should retain refresh status");
    assert_eq!(status.last_success_unix_seconds(), Some(200));
    assert_eq!(status.last_attempt_unix_seconds(), Some(250));
    assert_eq!(
        status.last_error_class(),
        Some(QuotaRefreshErrorClass::NetworkError)
    );
    let history = state
        .quota_history_observations_for_window(&account_id, "responses", 604_800, 0, 250)
        .await
        .expect("failure history should read");
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[1].refresh_outcome(),
        QuotaHistoryRefreshOutcome::Failure {
            error_class: QuotaRefreshErrorClass::NetworkError
        }
    );
}

#[tokio::test]
async fn pending_or_failed_newer_attempt_suppresses_cached_credit_without_retimestamping() {
    let temporary_directory = CreditStoreTempDir::new();
    let (state, account_id) =
        openai_account(&temporary_directory.database_path(), "pending", 1).await;
    let first_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("initial attempt should allocate");
    state
        .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
            attempt: &first_attempt,
            selector_windows: &[selector_window(&account_id, 0, 100)],
            observed_unix_seconds: 100,
            stale_after_unix_seconds: 460,
            provider_observation: &provider_observation(
                "2.50",
                CreditSpendControl::Unreported,
                Some(CreditProviderLimitReason::RateLimitReached),
            ),
            history_observations: &[successful_history_observation(&account_id, 0, 100)],
            snapshot: &quota_snapshot(&account_id, 0, 100),
        })
        .await
        .expect("initial observation should commit");

    state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("newer attempt sequence should be stored before provider IO");
    let observation = state
        .load_account_credit_observation(&account_id)
        .await
        .expect("pending observation should load")
        .expect("cached facts remain displayable while pending");

    assert_eq!(observation.latest_started_attempt(), 2);
    assert_eq!(observation.committed_attempt(), Some(1));
    assert_eq!(observation.observed_unix_seconds(), Some(100));
    assert!(
        !observation.authorizes_credit_usage(Some(1), 101),
        "an unfinished or failed newer attempt must suppress the older positive facts"
    );
}

#[tokio::test]
async fn latest_successful_missing_credit_facts_replace_previous_authority() {
    let temporary_directory = CreditStoreTempDir::new();
    let (state, account_id) = openai_account(
        &temporary_directory.database_path(),
        "missing_replacement",
        1,
    )
    .await;
    let first_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("initial attempt should allocate");
    state
        .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
            attempt: &first_attempt,
            selector_windows: &[selector_window(&account_id, 0, 100)],
            observed_unix_seconds: 100,
            stale_after_unix_seconds: 460,
            provider_observation: &provider_observation(
                "2.50",
                CreditSpendControl::Unreported,
                Some(CreditProviderLimitReason::RateLimitReached),
            ),
            history_observations: &[successful_history_observation(&account_id, 0, 100)],
            snapshot: &quota_snapshot(&account_id, 0, 100),
        })
        .await
        .expect("positive credit facts should commit first");

    let latest_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("latest refresh should allocate before provider IO");
    state
        .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
            attempt: &latest_attempt,
            selector_windows: &[selector_window(&account_id, 0, 101)],
            observed_unix_seconds: 101,
            stale_after_unix_seconds: 461,
            provider_observation: &CreditProviderObservation::missing(),
            history_observations: &[successful_history_observation(&account_id, 0, 101)],
            snapshot: &quota_snapshot(&account_id, 0, 101),
        })
        .await
        .expect("a valid quota response with absent credit facts should commit");

    let observation = state
        .load_account_credit_observation(&account_id)
        .await
        .expect("latest observation should read")
        .expect("refresh attempts should retain their generation/order row");
    assert_eq!(observation.latest_started_attempt(), 2);
    assert_eq!(observation.committed_attempt(), Some(2));
    assert_eq!(observation.observed_unix_seconds(), Some(101));
    assert_eq!(
        observation.provider_observation().availability(),
        &CreditAvailability::Unknown,
        "missing credit facts replace older positive facts"
    );
    assert!(!observation.authorizes_credit_usage(Some(1), 102));
}

#[tokio::test]
async fn credential_generation_change_invalidates_credit_and_rejects_old_attempt() {
    let temporary_directory = CreditStoreTempDir::new();
    let (state, account_id) =
        openai_account(&temporary_directory.database_path(), "generation", 1).await;
    let attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("attempt should bind generation one");
    state
        .activate_account_credential_generation_and_invalidate_quota(
            &account_id,
            2,
            AccountStatus::Enabled,
        )
        .await
        .expect("credential mutation should invalidate old quota and credits");

    assert!(
        !state
            .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
                attempt: &attempt,
                selector_windows: &[selector_window(&account_id, 0, 200)],
                observed_unix_seconds: 200,
                stale_after_unix_seconds: 560,
                provider_observation: &provider_observation(
                    "9.00",
                    CreditSpendControl::Clear,
                    Some(CreditProviderLimitReason::RateLimitReached),
                ),
                history_observations: &[successful_history_observation(&account_id, 0, 200)],
                snapshot: &quota_snapshot(&account_id, 0, 200),
            },)
            .await
            .expect("old-generation provider response should be rejected")
    );
    assert!(
        !state
            .record_responses_refresh_failure(
                &attempt,
                200,
                QuotaRefreshErrorClass::AuthError,
                &failed_history_observations(&account_id, 200, QuotaRefreshErrorClass::AuthError,),
            )
            .await
            .expect("old-generation resolver failure should be rejected")
    );
    let observation = state
        .load_account_credit_observation(&account_id)
        .await
        .expect("invalidated observation should read")
        .expect("generation marker should remain for monotonic sequencing");
    assert_eq!(observation.credential_generation(), 2);
    assert_eq!(
        observation.provider_observation().availability(),
        &CreditAvailability::Unknown
    );
    assert!(!observation.authorizes_credit_usage(Some(2), 201));

    let new_attempt = state
        .begin_credit_refresh_attempt(&account_id, 2)
        .await
        .expect("new credential generation should allocate next sequence");
    assert_eq!(new_attempt.sequence(), 2);
}

#[tokio::test]
async fn allocation_rejects_zero_sequence_without_repairing_corrupt_state() {
    let temporary_directory = CreditStoreTempDir::new();
    let (state, account_id) =
        openai_account(&temporary_directory.database_path(), "zero_sequence", 1).await;
    state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("initial attempt should create observation row");
    sqlx::query(
        "UPDATE account_credit_observations
                SET latest_started_attempt = 0
              WHERE account_id = ?1",
    )
    .bind(account_id.as_str())
    .execute(&state.pool)
    .await
    .expect("test should inject a corrupt zero sequence");

    assert_eq!(
        state.begin_credit_refresh_attempt(&account_id, 1).await,
        Err(StateStoreError::CorruptAccount {
            account_id: account_id.as_str().to_owned(),
            field: "latest_started_credit_attempt",
        }),
        "allocation must reject zero instead of silently incrementing corrupt state"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT latest_started_attempt
                   FROM account_credit_observations
                  WHERE account_id = ?1",
        )
        .bind(account_id.as_str())
        .fetch_one(&state.pool)
        .await
        .expect("corrupt sequence should remain available for diagnosis"),
        0
    );
}

#[tokio::test]
async fn allocation_reports_sequence_overflow_without_mutating_state() {
    let temporary_directory = CreditStoreTempDir::new();
    let (state, account_id) =
        openai_account(&temporary_directory.database_path(), "overflow", 1).await;
    state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("initial attempt should create observation row");
    sqlx::query(
        "UPDATE account_credit_observations
                SET latest_started_attempt = ?1
              WHERE account_id = ?2",
    )
    .bind(i64::MAX)
    .bind(account_id.as_str())
    .execute(&state.pool)
    .await
    .expect("test should inject the largest SQLite integer");

    assert_eq!(
        state.begin_credit_refresh_attempt(&account_id, 1).await,
        Err(StateStoreError::CreditRefreshAttemptSequenceOverflow)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT latest_started_attempt
                   FROM account_credit_observations
                  WHERE account_id = ?1",
        )
        .bind(account_id.as_str())
        .fetch_one(&state.pool)
        .await
        .expect("overflow sequence should remain unchanged"),
        i64::MAX
    );
}

#[tokio::test]
async fn observation_read_rejects_negative_attempt_sequences() {
    let temporary_directory = CreditStoreTempDir::new();
    let (latest_state, latest_account_id) = openai_account(
        &temporary_directory.path.join("negative_latest.sqlite"),
        "negative_latest",
        1,
    )
    .await;
    latest_state
        .begin_credit_refresh_attempt(&latest_account_id, 1)
        .await
        .expect("initial attempt should create observation row");
    sqlx::query(
        "UPDATE account_credit_observations
                SET latest_started_attempt = -1
              WHERE account_id = ?1",
    )
    .bind(latest_account_id.as_str())
    .execute(&latest_state.pool)
    .await
    .expect("test should inject a negative latest sequence");

    assert_eq!(
        latest_state
            .load_account_credit_observation(&latest_account_id)
            .await,
        Err(StateStoreError::CorruptAccount {
            account_id: latest_account_id.as_str().to_owned(),
            field: "latest_started_credit_attempt",
        })
    );

    let (committed_state, committed_account_id) = openai_account(
        &temporary_directory.path.join("negative_committed.sqlite"),
        "negative_committed",
        1,
    )
    .await;
    committed_state
        .begin_credit_refresh_attempt(&committed_account_id, 1)
        .await
        .expect("initial attempt should create observation row");
    sqlx::query(
        "UPDATE account_credit_observations
                SET committed_attempt = -1
              WHERE account_id = ?1",
    )
    .bind(committed_account_id.as_str())
    .execute(&committed_state.pool)
    .await
    .expect("test should inject a negative committed sequence");

    assert_eq!(
        committed_state
            .load_account_credit_observation(&committed_account_id)
            .await,
        Err(StateStoreError::CorruptAccount {
            account_id: committed_account_id.as_str().to_owned(),
            field: "committed_credit_attempt",
        })
    );
}
