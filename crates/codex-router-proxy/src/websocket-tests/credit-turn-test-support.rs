use super::*;

use crate::account_selection::AccountSourceAdmission;
use crate::account_selection::AsyncAccountSelectorRuntimeState;
use crate::account_selection::LiveAccountAdmissionAssessor;
use crate::account_selection::RouteBandAccountHolds;
use crate::account_selection::RouteBandQueueHealth;
use crate::account_selection::RouteBandReservationBooks;
use crate::account_selection::RouteBandRuntimeExhaustions;
use crate::account_selection::RouteBandWeightedSelectors;
use crate::account_selection::RuntimeAccountAdmissionAssessor;
use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditBalance;
use codex_router_core::credit_usage::CreditProviderLimitReason;
use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_core::credit_usage::CreditSpendControl;
use codex_router_core::credit_usage::CreditUsagePolicy;
use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaHistoryObservation;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::QuotaHistoryRefreshOutcome;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::SqliteStateStore;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

pub(super) const CREDIT_TURN_FIXTURE_TIME: u64 = 10_000;

static CREDIT_TURN_TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) struct CreditTurnTestDirectory {
    path: PathBuf,
}

impl CreditTurnTestDirectory {
    pub(super) fn new() -> Self {
        let sequence = CREDIT_TURN_TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "codex-router-credit-turn-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap_or_else(|error| {
            panic!("credit-turn test directory should be created: {error}")
        });
        Self { path }
    }

    pub(super) fn database_path(&self) -> PathBuf {
        self.path.join("state.sqlite")
    }
}

impl Drop for CreditTurnTestDirectory {
    fn drop(&mut self) {
        if self.path.exists() {
            fs::remove_dir_all(&self.path).unwrap_or_else(|error| {
                panic!("credit-turn test directory should be removed: {error}")
            });
        }
    }
}

pub(super) struct CreditTurnFixture {
    pub(super) account_id: AccountId,
    pub(super) database_path: PathBuf,
    pub(super) writer: AsyncSqliteStateStore,
    pub(super) reader: AsyncSqliteStateStore,
    pub(super) runtime_exhaustions: RouteBandRuntimeExhaustions,
    runtime_state: AsyncAccountSelectorRuntimeState,
    pub(super) assessor: RuntimeAccountAdmissionAssessor,
}

impl CreditTurnFixture {
    pub(super) async fn new(directory: &CreditTurnTestDirectory) -> Self {
        let database_path = directory.database_path();
        let account_id =
            AccountId::new("acct_credit_turn_source").expect("credit-turn account id should parse");
        let account = AccountRecord::new(
            Provider::Openai,
            account_id.clone(),
            "credit-turn-source",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        let synchronous_state =
            SqliteStateStore::open(&database_path).expect("credit-turn SQLite state should open");
        AccountStateRepository::upsert_account(&synchronous_state, &account)
            .expect("credit-turn account should persist");
        drop(synchronous_state);

        let writer = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("credit-turn async writer should open");
        writer
            .save_account_credit_usage_policy(&account_id, CreditUsagePolicy::Allow)
            .await
            .expect("credit policy should persist");
        let initial_observation = CreditProviderObservation::new(
            CreditAvailability::Available {
                balance: Some(CreditBalance::new("2.75").expect("credit balance is valid")),
            },
            CreditSpendControl::Clear,
            Some(CreditProviderLimitReason::RateLimitReached),
        );
        commit_responses_snapshot(
            &writer,
            &account_id,
            account.label(),
            credit_turn_windows(&account_id, CREDIT_TURN_FIXTURE_TIME),
            CREDIT_TURN_FIXTURE_TIME,
            &initial_observation,
        )
        .await;

        let reader = AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect("credit-turn read-only state should open");
        let runtime_exhaustions = RouteBandRuntimeExhaustions::default();
        let runtime_state = AsyncAccountSelectorRuntimeState::new(
            RouteBandWeightedSelectors::default(),
            RouteBandAccountHolds::default(),
            RouteBandReservationBooks::default(),
            Arc::clone(&runtime_exhaustions),
            RouteBandQueueHealth::default(),
        );
        let assessor = RuntimeAccountAdmissionAssessor::new(
            reader.clone(),
            &runtime_state,
            Arc::new(|| CREDIT_TURN_FIXTURE_TIME),
        );
        Self {
            account_id,
            database_path,
            writer,
            reader,
            runtime_exhaustions,
            runtime_state,
            assessor,
        }
    }

    pub(super) fn assessor_at(&self, now_unix_seconds: u64) -> RuntimeAccountAdmissionAssessor {
        RuntimeAccountAdmissionAssessor::new(
            self.reader.clone(),
            &self.runtime_state,
            Arc::new(move || now_unix_seconds),
        )
    }

    pub(super) fn selection_reservation_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        self.runtime_state.selection_reservation_lock_for_test()
    }

    pub(super) async fn replace_provider_observation(
        &self,
        provider_observation: &CreditProviderObservation,
    ) {
        self.replace_responses_snapshot(
            credit_turn_windows(&self.account_id, CREDIT_TURN_FIXTURE_TIME),
            provider_observation,
        )
        .await;
    }

    pub(super) async fn replace_responses_snapshot(
        &self,
        windows: Vec<PersistedSelectorQuotaWindow>,
        provider_observation: &CreditProviderObservation,
    ) {
        commit_responses_snapshot(
            &self.writer,
            &self.account_id,
            "credit-turn-source",
            windows,
            CREDIT_TURN_FIXTURE_TIME,
            provider_observation,
        )
        .await;
    }

    pub(super) async fn commit_included_quota(&self) {
        self.writer
            .save_account_credit_usage_policy(&self.account_id, CreditUsagePolicy::Disallow)
            .await
            .expect("credit opt-in should be disabled");
        let windows = vec![
            quota_window(
                &self.account_id,
                18_000,
                SelectorQuotaWindowStatus::Eligible,
                40,
                true,
                CREDIT_TURN_FIXTURE_TIME,
            ),
            quota_window(
                &self.account_id,
                604_800,
                SelectorQuotaWindowStatus::Eligible,
                20,
                false,
                CREDIT_TURN_FIXTURE_TIME,
            ),
        ];
        let missing_credit_authority = CreditProviderObservation::new(
            CreditAvailability::Unknown,
            CreditSpendControl::Unreported,
            None,
        );
        commit_responses_snapshot(
            &self.writer,
            &self.account_id,
            "credit-turn-source",
            windows,
            CREDIT_TURN_FIXTURE_TIME,
            &missing_credit_authority,
        )
        .await;
    }
}

pub(super) struct ObservedRuntimeAccountAssessor {
    pub(super) assessor: RuntimeAccountAdmissionAssessor,
    pub(super) observed_source_results: tokio::sync::mpsc::UnboundedSender<AccountSourceAdmission>,
}

impl LiveAccountAdmissionAssessor for ObservedRuntimeAccountAssessor {
    fn assess_peer<'a>(
        &'a self,
        source_account_id: &'a AccountId,
        route_band: RouteBand,
    ) -> BoxFuture<'a, crate::account_selection::FloorSwitchPeerAssessment> {
        self.assessor.assess_peer(source_account_id, route_band)
    }

    fn assess_source_account<'a>(
        &'a self,
        source_account_id: &'a AccountId,
        pinned_credential_generation: u64,
        route_band: RouteBand,
        credit_backed_admission_seen: bool,
    ) -> BoxFuture<'a, AccountSourceAdmission> {
        let result = self.assessor.assess_source_account(
            source_account_id,
            pinned_credential_generation,
            route_band,
            credit_backed_admission_seen,
        );
        let observed_source_results = self.observed_source_results.clone();
        Box::pin(async move {
            let assessment = result.await;
            let _recorded = observed_source_results.send(assessment);
            assessment
        })
    }
}

pub(super) async fn wait_for_source_assessment(
    source_results: &mut tokio::sync::mpsc::UnboundedReceiver<AccountSourceAdmission>,
) -> AccountSourceAdmission {
    tokio::time::timeout(std::time::Duration::from_secs(2), source_results.recv())
        .await
        .expect("source assessment should complete")
        .expect("source assessment event should be recorded")
}

pub(super) async fn next_client_text(
    client: &mut WebSocketStream<impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>,
) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(2), client.next())
        .await
        .expect("client frame should arrive")
        .expect("client frame should exist")
        .expect("client frame should decode")
        .to_string()
}

pub(super) fn credit_turn_windows(
    account_id: &AccountId,
    now_unix_seconds: u64,
) -> Vec<PersistedSelectorQuotaWindow> {
    vec![
        quota_window(
            account_id,
            18_000,
            SelectorQuotaWindowStatus::Ineligible,
            0,
            true,
            now_unix_seconds,
        ),
        quota_window(
            account_id,
            604_800,
            SelectorQuotaWindowStatus::Eligible,
            20,
            false,
            now_unix_seconds,
        ),
    ]
}

fn quota_window(
    account_id: &AccountId,
    window_seconds: u64,
    status: SelectorQuotaWindowStatus,
    remaining_headroom: u32,
    effective: bool,
    observed_unix_seconds: u64,
) -> PersistedSelectorQuotaWindow {
    PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        RouteBand::Responses.as_str(),
        window_seconds,
        status,
    )
    .with_remaining_headroom(remaining_headroom)
    .with_reset_unix_seconds(observed_unix_seconds + window_seconds)
    .with_effective(effective)
    .with_observed_unix_seconds(observed_unix_seconds)
}

async fn commit_responses_snapshot(
    writer: &AsyncSqliteStateStore,
    account_id: &AccountId,
    account_label: &str,
    windows: Vec<PersistedSelectorQuotaWindow>,
    observed_unix_seconds: u64,
    provider_observation: &CreditProviderObservation,
) {
    let attempt = writer
        .begin_credit_refresh_attempt(account_id, 1)
        .await
        .expect("credit refresh attempt should begin");
    let history = windows
        .iter()
        .map(|window| {
            PersistedQuotaHistoryObservation::new(
                account_id.clone(),
                account_label,
                RouteBand::Responses.as_str(),
                window.limit_window_seconds(),
                observed_unix_seconds,
                window.remaining_headroom(),
            )
            .with_reset_unix_seconds(
                window
                    .reset_unix_seconds()
                    .expect("credit-turn window reset should exist"),
            )
            .with_window_status(window.status())
            .with_effective(window.effective())
            .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
            .with_refresh_outcome(QuotaHistoryRefreshOutcome::Success)
        })
        .collect::<Vec<_>>();
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band(RouteBand::Responses.as_str(), 0)
            .with_reset_unix_seconds(observed_unix_seconds + 18_000)
            .with_stale_penalty(false);
    let committed = writer
        .record_responses_refresh_success(
            codex_router_state::credit_store::ResponsesRefreshSuccessCommit {
                attempt: &attempt,
                selector_windows: &windows,
                observed_unix_seconds,
                stale_after_unix_seconds: observed_unix_seconds + 300,
                provider_observation,
                history_observations: &history,
                snapshot: &snapshot,
            },
        )
        .await
        .expect("quota and credit observation should commit");
    assert!(committed, "credit attempt should remain current");
}
