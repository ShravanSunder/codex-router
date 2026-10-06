use super::*;

struct RecoveringClaudeQuotaCredentialResolver {
    recovery_calls: std::sync::atomic::AtomicUsize,
}

impl crate::credential_runtime::AsyncProviderCredentialResolver
    for RecoveringClaudeQuotaCredentialResolver
{
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        if expected_provider != codex_router_core::provider::Provider::Claude {
            return Err(CredentialResolverError::AccountProviderMismatch);
        }
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            SecretString::new("claude-quota-old-access"),
            1,
        ))
    }

    async fn recover_unauthorized_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
        rejected_generation: u64,
    ) -> Result<(ResolvedProviderCredential, bool), CredentialResolverError> {
        if expected_provider != codex_router_core::provider::Provider::Claude
            || rejected_generation != 1
        {
            return Err(CredentialResolverError::AccountIneligible);
        }
        self.recovery_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok((
            ResolvedProviderCredential::new(
                account_id.clone(),
                SecretString::new("claude-quota-recovered-access"),
                2,
            ),
            true,
        ))
    }
}

struct RecordingClaudeQuotaProvider {
    unauthorized_once: bool,
    requests: std::sync::Mutex<Vec<(String, String)>>,
}

impl RecordingClaudeQuotaProvider {
    fn new(unauthorized_once: bool) -> Self {
        Self {
            unauthorized_once,
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn recorded_requests(&self) -> Vec<(String, String)> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl QuotaRefreshProvider for RecordingClaudeQuotaProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        let call_number = {
            let mut requests = self
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            requests.push((
                request.account_id().as_str().to_owned(),
                request.access_token().expose_secret().to_owned(),
            ));
            requests.len()
        };
        if self.unauthorized_once && call_number == 1 {
            return Err(crate::quota::QuotaCommandError::ProviderStatus { status: 401 });
        }
        Ok(QuotaRefreshProviderResponse {
            windows: vec![QuotaRefreshProviderWindow {
                limit_window_seconds: 18_000,
                headroom: crate::quota::QuotaWindowHeadroom::BasisPoints(7_500),
                reset_unix_seconds: Some(20_000),
                effective: true,
            }],
            reset_credits_available: None,
            credit_provider_observation:
                codex_router_core::credit_usage::CreditProviderObservation::missing(),
        })
    }
}

#[test]
fn claude_quota_401_recovers_credentials_and_retries_the_usage_request() {
    use codex_router_core::provider::Provider;
    use codex_router_state::sqlite::AsyncSqliteStateStore;
    use std::sync::atomic::Ordering;

    let test_root = TestRoot::new("claude-quota-401-recovery");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let account_id = account_id("acct_claude_401_recovery");
    let state = must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(
        test_async_runtime().block_on(
            state.upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "claude-401",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            ),
        ),
    );
    must_ok(test_async_runtime().block_on(state.close()));

    let resolver = RecoveringClaudeQuotaCredentialResolver {
        recovery_calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let provider = RecordingClaudeQuotaProvider::new(true);
    let mut stdout = Vec::new();
    must_ok(
        test_async_runtime().block_on(refresh_quota_store_paths_with_dependencies_async(
            &mut stdout,
            &state_path,
            &router_root.join("secrets"),
            "unused-openai-base-url".to_owned(),
            &resolver,
            &provider,
            2_000,
        )),
    );

    assert_eq!(resolver.recovery_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider.recorded_requests(),
        vec![
            (
                account_id.as_str().to_owned(),
                "claude-quota-old-access".to_owned(),
            ),
            (
                account_id.as_str().to_owned(),
                "claude-quota-recovered-access".to_owned(),
            ),
        ]
    );
    assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 1\n");
}

#[test]
fn background_quota_refresh_polls_only_claude_accounts_idle_longer_than_interval() {
    use codex_router_core::ids::ReservationId;
    use codex_router_core::provider::Provider;
    use codex_router_state::sqlite::AsyncSqliteStateStore;

    let test_root = TestRoot::new("claude-background-quota-idle-only");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let state = must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(&state_path)));
    let labels = ["busy", "recent", "idle", "never-used"];
    let accounts = labels.map(|label| {
        AccountRecord::new(
            Provider::Claude,
            account_id(&format!("acct_claude_{label}")),
            label,
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1)
    });
    for account in &accounts {
        must_ok(test_async_runtime().block_on(state.upsert_account(account)));
    }
    let busy_reservation = ReservationId::new("busy-reservation");
    must_ok(
        test_async_runtime().block_on(state.record_active_client_acquired(
            "claude_messages",
            "busy-process",
            &busy_reservation,
            accounts[0].account_id(),
            1_000,
            0,
        )),
    );
    for (account_index, acquired_at, released_at, process_id) in
        [(1, 1_600, 1_800, "recent"), (2, 1_000, 1_100, "idle")]
    {
        let reservation = ReservationId::new(format!("{process_id}-reservation"));
        must_ok(
            test_async_runtime().block_on(state.record_active_client_acquired(
                "claude_messages",
                process_id,
                &reservation,
                accounts[account_index].account_id(),
                acquired_at,
                0,
            )),
        );
        must_ok(
            test_async_runtime().block_on(state.record_active_client_released(
                "claude_messages",
                process_id,
                &reservation,
                released_at,
            )),
        );
    }
    must_ok(test_async_runtime().block_on(state.close()));

    let provider = RecordingClaudeQuotaProvider::new(false);
    let resolver = FixedClaudeQuotaCredentialResolver;
    let mut stdout = Vec::new();
    must_ok(test_async_runtime().block_on(
        crate::quota::refresh_quota_store_paths_with_dependencies_and_floor_notifier(
            &mut stdout,
            &state_path,
            &router_root.join("secrets"),
            "unused-openai-base-url".to_owned(),
            &resolver,
            &provider,
            crate::quota::QuotaRefreshObservationContext {
                observed_unix_seconds: 2_000,
                schedule: crate::quota::QuotaRefreshSchedule::Background {
                    interval_seconds: 400,
                },
                weekly_floor_observer: None,
            },
        ),
    ));

    let polled_account_ids = provider
        .recorded_requests()
        .into_iter()
        .map(|(account_id, _token)| account_id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        polled_account_ids,
        std::collections::HashSet::from([
            accounts[2].account_id().as_str().to_owned(),
            accounts[3].account_id().as_str().to_owned(),
        ])
    );
    assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");
}
