use super::*;

pub(super) fn persist_credit_backed_account_with_token(
    database_path: &Path,
    secrets: &EncryptedCredentialStore,
    account: &AccountRecord,
    upstream_token: &str,
    observed_unix_seconds: u64,
    allow_credit_usage: bool,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("credit fixture runtime should build: {error}"));
    runtime.block_on(persist_credit_backed_account_with_token_async(
        database_path,
        secrets,
        account,
        upstream_token,
        observed_unix_seconds,
        allow_credit_usage,
    ));
}

pub(super) async fn persist_credit_backed_account_with_token_async(
    database_path: &Path,
    secrets: &EncryptedCredentialStore,
    account: &AccountRecord,
    upstream_token: &str,
    observed_unix_seconds: u64,
    allow_credit_usage: bool,
) {
    persist_credit_account_with_availability_async(
        database_path,
        secrets,
        account,
        upstream_token,
        observed_unix_seconds,
        allow_credit_usage,
        codex_router_core::credit_usage::CreditAvailability::Available {
            balance: Some(
                codex_router_core::credit_usage::CreditBalance::new("2.75")
                    .expect("fixture credit balance should be valid"),
            ),
        },
    )
    .await;
}

pub(super) fn persist_credit_account_with_availability(
    database_path: &Path,
    secrets: &EncryptedCredentialStore,
    account: &AccountRecord,
    upstream_token: &str,
    observed_unix_seconds: u64,
    allow_credit_usage: bool,
    availability: codex_router_core::credit_usage::CreditAvailability,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("credit fixture runtime should build: {error}"));
    runtime.block_on(persist_credit_account_with_availability_async(
        database_path,
        secrets,
        account,
        upstream_token,
        observed_unix_seconds,
        allow_credit_usage,
        availability,
    ));
}

async fn persist_credit_account_with_availability_async(
    database_path: &Path,
    secrets: &EncryptedCredentialStore,
    account: &AccountRecord,
    upstream_token: &str,
    observed_unix_seconds: u64,
    allow_credit_usage: bool,
    availability: codex_router_core::credit_usage::CreditAvailability,
) {
    let account_with_generation = account.clone().with_active_credential_generation(1);
    let sync_state = SqliteStateStore::open(database_path)
        .unwrap_or_else(|error| panic!("state should open for credit account: {error}"));
    AccountStateRepository::upsert_account(&sync_state, &account_with_generation)
        .unwrap_or_else(|error| panic!("credit account should persist: {error}"));

    let async_state = AsyncSqliteStateStore::open(database_path)
        .await
        .unwrap_or_else(|error| panic!("credit state should open: {error}"));
    async_state
        .save_account_credit_usage_policy(
            account.account_id(),
            if allow_credit_usage {
                codex_router_core::credit_usage::CreditUsagePolicy::Allow
            } else {
                codex_router_core::credit_usage::CreditUsagePolicy::Disallow
            },
        )
        .await
        .unwrap_or_else(|error| panic!("credit policy should persist: {error}"));
    let attempt = async_state
        .begin_credit_refresh_attempt(account.account_id(), 1)
        .await
        .unwrap_or_else(|error| panic!("credit attempt should allocate: {error}"));
    let windows = [
        PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Ineligible,
        )
        .with_remaining_headroom(0)
        .with_effective(true)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_reset_unix_seconds(observed_unix_seconds + 18_000),
        PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Ineligible,
        )
        .with_remaining_headroom(0)
        .with_effective(false)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_reset_unix_seconds(observed_unix_seconds + 604_800),
    ];
    let history = windows
        .iter()
        .map(|window| {
            PersistedQuotaHistoryObservation::new(
                account.account_id().clone(),
                account.label(),
                "responses",
                window.limit_window_seconds(),
                observed_unix_seconds,
                window.remaining_headroom(),
            )
            .with_reset_unix_seconds(
                window
                    .reset_unix_seconds()
                    .expect("credit fixture windows should have reset times"),
            )
            .with_window_status(SelectorQuotaWindowStatus::Ineligible)
            .with_effective(window.effective())
            .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
            .with_refresh_outcome(QuotaHistoryRefreshOutcome::Success)
        })
        .collect::<Vec<_>>();
    let snapshot = PersistedQuotaSnapshot::new(
        account.account_id().clone(),
        QuotaSnapshotSource::OpenAiEndpoint,
    )
    .with_observed_unix_seconds(observed_unix_seconds)
    .with_route_band("responses", 0)
    .with_reset_unix_seconds(observed_unix_seconds + 18_000)
    .with_stale_penalty(false);
    let observation = codex_router_core::credit_usage::CreditProviderObservation::new(
        availability,
        codex_router_core::credit_usage::CreditSpendControl::Clear,
        Some(codex_router_core::credit_usage::CreditProviderLimitReason::RateLimitReached),
    );
    let committed = async_state
        .record_responses_refresh_success(
            codex_router_state::credit_store::ResponsesRefreshSuccessCommit {
                attempt: &attempt,
                selector_windows: &windows,
                observed_unix_seconds,
                stale_after_unix_seconds: observed_unix_seconds + 300,
                provider_observation: &observation,
                history_observations: &history,
                snapshot: &snapshot,
            },
        )
        .await
        .unwrap_or_else(|error| panic!("coherent credit fixture should commit: {error}"));
    assert!(committed, "credit fixture should be latest attempt");
    async_state
        .close()
        .await
        .unwrap_or_else(|error| panic!("credit state should close: {error}"));

    let token_key = openai_account_credential_bundle_key(account.account_id(), 1)
        .unwrap_or_else(|error| panic!("credit token key should build: {error}"));
    let bundle = AccountCredentialBundle::imported_codex_auth(
        upstream_token,
        Some(format!("{upstream_token}-refresh")),
    )
    .to_secret_string()
    .unwrap_or_else(|error| panic!("credit token should serialize: {error}"));
    secrets
        .write_secret(&token_key, &bundle)
        .unwrap_or_else(|error| panic!("credit token should persist: {error}"));
}
