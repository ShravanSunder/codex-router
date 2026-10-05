use super::*;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_core::route_profile::WindowKind;
use codex_router_state::quota_snapshot::QuotaRefreshStatusView;

struct SyntheticClaudeCredentialResolver;

impl AsyncProviderCredentialResolver for SyntheticClaudeCredentialResolver {
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        if expected_provider != Provider::Claude {
            return Err(CredentialResolverError::AccountProviderMismatch);
        }
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            SecretString::new("synthetic-claude-quota-credential"),
            1,
        ))
    }
}

enum SyntheticClaudeQuotaResponse {
    ParseError,
    Windows(Vec<QuotaRefreshProviderWindow>),
    RejectMetadataWrite(PathBuf),
}

impl QuotaRefreshProvider for SyntheticClaudeQuotaResponse {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
        if request.provider() != Provider::Claude
            || request.route_band() != RouteBand::ClaudeMessages.as_str()
        {
            return Err(QuotaCommandError::ProviderResponse {
                message: format!(
                    "synthetic quota provider expected claude/claude_messages, received {}/{}",
                    request.provider().as_str(),
                    request.route_band(),
                ),
            });
        }
        match self {
            Self::ParseError => Err(QuotaCommandError::ProviderResponse {
                message: "synthetic quota parse failure".to_owned(),
            }),
            Self::Windows(windows) => Ok(QuotaRefreshProviderResponse {
                windows: windows.clone(),
                ..Default::default()
            }),
            Self::RejectMetadataWrite(database_path) => {
                let pool = fixture_sqlite_pool(database_path).await;
                sqlx::query(
                    "CREATE TRIGGER reject_success_metadata BEFORE INSERT ON quota_refresh_status
                     WHEN NEW.last_error_class IS NULL
                     BEGIN SELECT RAISE(ABORT, 'synthetic metadata write failure'); END",
                )
                .execute(&pool)
                .await
                .expect("synthetic metadata failure after schema admission");
                pool.close().await;
                Ok(QuotaRefreshProviderResponse {
                    windows: successful_claude_windows(),
                    ..Default::default()
                })
            }
        }
    }
}

async fn fixture_sqlite_pool(database_path: &Path) -> sqlx::SqlitePool {
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(database_path))
        .await
        .expect("fixture connection")
}

fn successful_claude_windows() -> Vec<QuotaRefreshProviderWindow> {
    vec![
        QuotaRefreshProviderWindow {
            limit_window_seconds: V1_SHORT_WINDOW_SECONDS,
            headroom: QuotaWindowHeadroom::BasisPoints(6_000),
            reset_unix_seconds: Some(19_000),
            effective: true,
        },
        QuotaRefreshProviderWindow {
            limit_window_seconds: V1_WEEKLY_WINDOW_SECONDS,
            headroom: QuotaWindowHeadroom::BasisPoints(7_000),
            reset_unix_seconds: Some(605_000),
            effective: true,
        },
    ]
}

struct ClaudeRefreshFixture {
    _test_root: TempDir,
    router_root: PathBuf,
    account_id: AccountId,
}

impl ClaudeRefreshFixture {
    async fn new() -> Self {
        let test_root = TempDir::new().expect("quota recovery root");
        let router_root = test_root.path().join("router");
        std::fs::create_dir_all(&router_root).expect("router root");
        let account_id = AccountId::new("claude_refresh_recovery").expect("Claude account id");
        let state = AsyncSqliteStateStore::open(&router_root.join("state.sqlite"))
            .await
            .expect("state store should open");
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "claude-recovery",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await
            .expect("Claude account should persist");
        state.close().await.expect("state store should close");
        Self {
            _test_root: test_root,
            router_root,
            account_id,
        }
    }

    async fn refresh(
        &self,
        response: SyntheticClaudeQuotaResponse,
        observed_unix_seconds: u64,
    ) -> (Result<QuotaRefreshReport, QuotaCommandError>, String) {
        let mut stdout = Vec::new();
        let result = refresh_quota_with_dependencies(
            &mut stdout,
            self.router_root.clone(),
            "unused-synthetic-provider-url".to_owned(),
            &SyntheticClaudeCredentialResolver,
            &response,
            observed_unix_seconds,
        )
        .await;
        (result, String::from_utf8(stdout).expect("refresh output"))
    }

    async fn report(&self, now_unix_seconds: u64) -> QuotaStatusReport {
        load_quota_status_report_with_availability_async(
            &self.router_root,
            false,
            now_unix_seconds,
            false,
            CredentialStoreAvailability::Ready,
        )
        .await
        .expect("report should read persisted producer state")
    }

    async fn read_observations_and_status(
        &self,
    ) -> (Vec<WindowObservation>, Option<QuotaRefreshStatusView>) {
        let state = AsyncSqliteStateStore::open_read_only(&self.router_root.join("state.sqlite"))
            .await
            .expect("read state");
        let observations = state
            .window_observations_for_account(&self.account_id)
            .await
            .expect("read observations");
        let status = state
            .quota_refresh_statuses_for_route_band(RouteBand::ClaudeMessages.as_str())
            .await
            .expect("read refresh status")
            .into_iter()
            .find(|status| status.account_id() == &self.account_id);
        state.close().await.expect("close read state");
        (observations, status)
    }
}

#[tokio::test]
async fn actual_claude_refresh_success_clears_failure_and_records_observation_time() {
    let fixture = ClaudeRefreshFixture::new().await;
    let (result, _) = fixture
        .refresh(SyntheticClaudeQuotaResponse::ParseError, 1_000)
        .await;
    assert!(result.is_err());
    let failed_report = fixture.report(1_001).await;
    let failed_row = failed_report
        .rows()
        .iter()
        .find(|row| row.account_id == fixture.account_id)
        .expect("Claude failure row");
    assert!(failed_row.updated.contains("no success"));
    assert!(failed_row.updated.contains(": parse"));

    let (result, stdout) = fixture
        .refresh(
            SyntheticClaudeQuotaResponse::Windows(successful_claude_windows()),
            2_000,
        )
        .await;
    result.expect("accepted Claude observations should refresh successfully");
    assert_eq!(stdout, "refreshed: 1\n");
    let (accepted_observations, status) = fixture.read_observations_and_status().await;
    assert_eq!(accepted_observations.len(), 2);
    let observation_started_at = accepted_observations[0].observation_started_at();
    let fresh_until = accepted_observations[0]
        .fresh_until_unix_seconds()
        .expect("persisted deadline");
    let status = status.expect("successful refresh status should be recorded");
    assert_eq!(status.last_error_class(), None);
    assert_eq!(
        status.last_success_unix_seconds(),
        Some(observation_started_at)
    );
    assert_eq!(
        status.last_attempt_unix_seconds(),
        Some(observation_started_at)
    );
    assert_eq!(status.stale_after_unix_seconds(), Some(fresh_until));
    assert_eq!(
        fresh_until,
        observation_started_at + crate::DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS + 120
    );
    for (now_unix_seconds, expected_freshness) in [
        (observation_started_at, QuotaEvidenceFreshness::Fresh),
        (fresh_until, QuotaEvidenceFreshness::Fresh),
        (fresh_until + 1, QuotaEvidenceFreshness::Stale),
    ] {
        let report = fixture.report(now_unix_seconds).await;
        let row = report
            .rows()
            .iter()
            .find(|row| row.account_id == fixture.account_id)
            .expect("Claude recovery row");
        assert!(row.updated.starts_with("ok "), "{}", row.updated);
        assert!(!row.updated.contains("failed"), "{}", row.updated);
        assert!(!row.updated.contains("no success"), "{}", row.updated);
        assert_eq!(row.freshness, expected_freshness);
        assert_eq!(row.credit_usage.freshness, CreditUsageFreshness::Unknown);
        assert_eq!(row.windows.len(), 2);
        for (window_seconds, expected_headroom) in [
            (V1_SHORT_WINDOW_SECONDS, 60),
            (V1_WEEKLY_WINDOW_SECONDS, 70),
        ] {
            let window = row
                .windows
                .iter()
                .find(|window| window.window_seconds == window_seconds)
                .expect("accepted window should appear");
            assert_eq!(window.remaining_headroom, expected_headroom);
            assert_eq!(window.observed_unix_seconds, observation_started_at);
        }
    }

    let (result, _) = fixture
        .refresh(SyntheticClaudeQuotaResponse::ParseError, fresh_until + 1)
        .await;
    assert!(result.is_err());
    let (preserved_observations, failed_status) = fixture.read_observations_and_status().await;
    assert_eq!(preserved_observations, accepted_observations);
    let failed_status = failed_status.expect("later failure status");
    assert_eq!(
        failed_status.last_success_unix_seconds(),
        Some(observation_started_at)
    );
    assert_eq!(
        failed_status.last_attempt_unix_seconds(),
        Some(fresh_until + 1)
    );
    assert_eq!(
        failed_status.last_error_class(),
        Some(QuotaRefreshErrorClass::ParseError)
    );
}

#[tokio::test]
async fn claude_refresh_without_accepted_observations_preserves_failure_status() {
    for response in [
        SyntheticClaudeQuotaResponse::Windows(Vec::new()),
        SyntheticClaudeQuotaResponse::Windows(successful_claude_windows()),
    ] {
        let fixture = ClaudeRefreshFixture::new().await;
        let state = AsyncSqliteStateStore::open(&fixture.router_root.join("state.sqlite"))
            .await
            .expect("state");
        let future_start = current_unix_seconds() + 10_000;
        for window_kind in [WindowKind::FiveHour, WindowKind::Weekly] {
            let observation = WindowObservation::new(
                WindowObservationProps::new(
                    fixture.account_id.clone(),
                    window_kind,
                    8_000,
                    future_start,
                )
                .with_fresh_until_unix_seconds(future_start + 500),
            )
            .expect("future observation");
            state
                .record_window_observation(&observation, || future_start)
                .await
                .expect("newer observation should persist");
        }
        state.close().await.expect("close state");
        let (result, _) = fixture
            .refresh(SyntheticClaudeQuotaResponse::ParseError, 1_000)
            .await;
        assert!(result.is_err());
        let (original_observations, original_status) = fixture.read_observations_and_status().await;

        let (result, stdout) = fixture.refresh(response, 2_000).await;
        result.expect("no accepted observations is not a provider failure");
        assert_eq!(stdout, "refreshed: 0\n");
        let (observations, status) = fixture.read_observations_and_status().await;
        assert_eq!(observations, original_observations);
        assert_eq!(status, original_status);
    }
}

#[tokio::test]
async fn claude_refresh_metadata_write_failure_is_not_reported_as_success() {
    let fixture = ClaudeRefreshFixture::new().await;
    let database_path = fixture.router_root.join("state.sqlite");
    let (result, stdout) = fixture
        .refresh(
            SyntheticClaudeQuotaResponse::RejectMetadataWrite(database_path.clone()),
            2_000,
        )
        .await;
    let pool = fixture_sqlite_pool(&database_path).await;
    sqlx::query("DROP TRIGGER reject_success_metadata")
        .execute(&pool)
        .await
        .expect("restore synthetic schema before read-only state admission");
    pool.close().await;
    assert!(
        matches!(result, Err(QuotaCommandError::StateStore(_))),
        "{result:?}"
    );
    assert!(!stdout.contains("refreshed: 1"));
    let (observations, status) = fixture.read_observations_and_status().await;
    assert_eq!(observations.len(), 2);
    assert!(status.is_none());
}
