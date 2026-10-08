use super::*;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

struct InitialCredentialResolutionFailure;

impl AsyncProviderCredentialResolver for InitialCredentialResolutionFailure {
    async fn resolve_provider_credentials_async(
        &self,
        _account_id: &AccountId,
        _expected_provider: Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        Err(CredentialResolverError::RefreshUnavailable)
    }
}

#[derive(Clone, Copy)]
enum UnauthorizedRecovery {
    Fail,
    Renew,
}

struct UnauthorizedRecoveryResolver {
    recovery: UnauthorizedRecovery,
}

impl AsyncProviderCredentialResolver for UnauthorizedRecoveryResolver {
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

    async fn recover_unauthorized_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
        _rejected_generation: u64,
    ) -> Result<(ResolvedProviderCredential, bool), CredentialResolverError> {
        if expected_provider != Provider::Claude {
            return Err(CredentialResolverError::AccountProviderMismatch);
        }
        match self.recovery {
            UnauthorizedRecovery::Fail => Err(CredentialResolverError::RefreshUnavailable),
            UnauthorizedRecovery::Renew => Ok((
                ResolvedProviderCredential::new(
                    account_id.clone(),
                    SecretString::new("synthetic-renewed-claude-quota-credential"),
                    2,
                ),
                true,
            )),
        }
    }
}

struct SequencedClaudeQuotaProvider {
    responses: Mutex<VecDeque<Result<QuotaRefreshProviderResponse, QuotaCommandError>>>,
    calls: AtomicUsize,
}

impl SequencedClaudeQuotaProvider {
    fn new(responses: Vec<Result<QuotaRefreshProviderResponse, QuotaCommandError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl QuotaRefreshProvider for SequencedClaudeQuotaProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
        assert_eq!(request.provider(), Provider::Claude);
        assert_eq!(request.route_band(), RouteBand::ClaudeMessages.as_str());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .expect("synthetic response queue")
            .pop_front()
            .expect("one synthetic response per provider call")
    }
}

async fn seed_successful_claude_poll(fixture: &ClaudeRefreshFixture) -> u64 {
    let (result, stdout) = fixture
        .refresh(
            SyntheticClaudeQuotaResponse::Windows(successful_claude_windows()),
            1_000,
        )
        .await;
    result.expect("initial Claude poll should succeed");
    assert_eq!(stdout, "refreshed: 1\n");
    let (_, status) = fixture.read_observations_and_status().await;
    status
        .expect("successful poll records refresh status")
        .last_attempt_unix_seconds()
        .expect("successful poll records its attempt time")
}

async fn refresh_with_dependencies<R, P>(
    fixture: &ClaudeRefreshFixture,
    resolver: &R,
    provider: &P,
    supplied_cycle_time: u64,
) -> Result<QuotaRefreshReport, QuotaCommandError>
where
    R: AsyncProviderCredentialResolver,
    P: QuotaRefreshProvider,
{
    refresh_quota_with_dependencies(
        &mut Vec::new(),
        fixture.router_root.clone(),
        "unused-synthetic-provider-url".to_owned(),
        resolver,
        provider,
        supplied_cycle_time,
    )
    .await
}

fn assert_actual_attempt_in_bounds(attempt: Option<u64>, before: u64, after: u64) {
    let attempt = attempt.expect("failed operation records its attempt time");
    assert!(
        attempt >= before,
        "attempt {attempt} predates operation bound {before}"
    );
    assert!(
        attempt <= after,
        "attempt {attempt} exceeds operation bound {after}"
    );
}

#[tokio::test]
async fn older_cycle_time_does_not_hide_actual_claude_parse_failure() {
    let fixture = ClaudeRefreshFixture::new().await;
    let successful_attempt = seed_successful_claude_poll(&fixture).await;
    let supplied_cycle_time = successful_attempt
        .checked_sub(1)
        .expect("current attempt time is positive");
    let poll_started_before = current_unix_seconds();
    assert!(supplied_cycle_time < poll_started_before);

    let (result, _) = fixture
        .refresh(
            SyntheticClaudeQuotaResponse::ParseError,
            supplied_cycle_time,
        )
        .await;
    let poll_finished_after = current_unix_seconds();

    assert!(result.is_err(), "parse failure remains an error");
    let (_, status) = fixture.read_observations_and_status().await;
    let status = status.expect("parse failure keeps current account status");
    assert_eq!(
        status.last_success_unix_seconds(),
        Some(successful_attempt),
        "last successful poll remains visible"
    );
    assert_actual_attempt_in_bounds(
        status.last_attempt_unix_seconds(),
        poll_started_before,
        poll_finished_after,
    );
    assert_eq!(
        status.last_error_class(),
        Some(QuotaRefreshErrorClass::ParseError),
        "the later poll's parse failure replaces the stale cycle timestamp"
    );

    let report = fixture.report(poll_finished_after).await;
    let row = report
        .rows()
        .iter()
        .find(|row| row.account_id == fixture.account_id)
        .expect("current Claude account appears in report");
    assert!(row.updated.contains("failed"), "{}", row.updated);
    assert!(row.updated.contains(": parse"), "{}", row.updated);
}

#[tokio::test]
async fn older_cycle_time_does_not_hide_initial_credential_resolution_failure() {
    let fixture = ClaudeRefreshFixture::new().await;
    let successful_attempt = seed_successful_claude_poll(&fixture).await;
    let supplied_cycle_time = successful_attempt
        .checked_sub(1)
        .expect("current attempt time is positive");
    let provider = SequencedClaudeQuotaProvider::new(Vec::new());
    let resolver = InitialCredentialResolutionFailure;
    let resolver_started_before = current_unix_seconds();
    assert!(supplied_cycle_time < resolver_started_before);

    let result =
        refresh_with_dependencies(&fixture, &resolver, &provider, supplied_cycle_time).await;
    let resolver_finished_after = current_unix_seconds();

    assert!(
        result.is_err(),
        "credential resolution failure remains an error"
    );
    assert_eq!(
        provider.calls(),
        0,
        "provider is not called without credentials"
    );
    let (_, status) = fixture.read_observations_and_status().await;
    let status = status.expect("credential failure keeps current account status");
    assert_eq!(status.last_success_unix_seconds(), Some(successful_attempt));
    assert_actual_attempt_in_bounds(
        status.last_attempt_unix_seconds(),
        resolver_started_before,
        resolver_finished_after,
    );
    assert_eq!(
        status.last_error_class(),
        Some(QuotaRefreshErrorClass::AuthError)
    );

    let report = fixture.report(resolver_finished_after).await;
    let row = report
        .rows()
        .iter()
        .find(|row| row.account_id == fixture.account_id)
        .expect("current Claude account appears in report");
    assert!(row.updated.contains(": auth"), "{}", row.updated);
}

#[tokio::test]
async fn failed_claude_401_recovery_records_auth_error_at_poll_start() {
    let fixture = ClaudeRefreshFixture::new().await;
    let successful_attempt = seed_successful_claude_poll(&fixture).await;
    let supplied_cycle_time = successful_attempt
        .checked_sub(1)
        .expect("current attempt time is positive");
    let resolver = UnauthorizedRecoveryResolver {
        recovery: UnauthorizedRecovery::Fail,
    };
    let provider =
        SequencedClaudeQuotaProvider::new(vec![Err(QuotaCommandError::ProviderStatus {
            status: 401,
        })]);
    let poll_started_before = current_unix_seconds();
    assert!(supplied_cycle_time < poll_started_before);

    let result =
        refresh_with_dependencies(&fixture, &resolver, &provider, supplied_cycle_time).await;
    let poll_finished_after = current_unix_seconds();

    assert!(result.is_err(), "failed 401 recovery remains an error");
    assert_eq!(
        provider.calls(),
        1,
        "failed recovery does not retry the provider"
    );
    let (_, status) = fixture.read_observations_and_status().await;
    let status = status.expect("401 recovery failure status");
    assert_eq!(status.last_success_unix_seconds(), Some(successful_attempt));
    assert_actual_attempt_in_bounds(
        status.last_attempt_unix_seconds(),
        poll_started_before,
        poll_finished_after,
    );
    assert_eq!(
        status.last_error_class(),
        Some(QuotaRefreshErrorClass::AuthError)
    );
}

#[tokio::test]
async fn final_claude_retry_error_records_rate_limit_at_poll_start() {
    let fixture = ClaudeRefreshFixture::new().await;
    let successful_attempt = seed_successful_claude_poll(&fixture).await;
    let supplied_cycle_time = successful_attempt
        .checked_sub(1)
        .expect("current attempt time is positive");
    let resolver = UnauthorizedRecoveryResolver {
        recovery: UnauthorizedRecovery::Renew,
    };
    let provider = SequencedClaudeQuotaProvider::new(vec![
        Err(QuotaCommandError::ProviderStatus { status: 401 }),
        Err(QuotaCommandError::ProviderStatus { status: 429 }),
    ]);
    let poll_started_before = current_unix_seconds();
    assert!(supplied_cycle_time < poll_started_before);

    let result =
        refresh_with_dependencies(&fixture, &resolver, &provider, supplied_cycle_time).await;
    let poll_finished_after = current_unix_seconds();

    assert!(result.is_err(), "final retry error remains an error");
    assert_eq!(provider.calls(), 2, "one retry follows successful renewal");
    let (_, status) = fixture.read_observations_and_status().await;
    let status = status.expect("final retry failure status");
    assert_eq!(status.last_success_unix_seconds(), Some(successful_attempt));
    assert_actual_attempt_in_bounds(
        status.last_attempt_unix_seconds(),
        poll_started_before,
        poll_finished_after,
    );
    assert_eq!(
        status.last_error_class(),
        Some(QuotaRefreshErrorClass::RateLimited)
    );
}
