use super::UpkeepCycleResult;
use super::bounded_upkeep_wait;
use super::*;
use codex_router_auth::resolver::NoopCredentialRefreshClient;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::openai_account_credential_bundle_key;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_state::account::AccountRecord;
use codex_router_state::credential_maintenance::CredentialFailureClass;
use codex_router_state::credential_maintenance::CredentialMaintenanceState;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[tokio::test]
async fn unavailable_credential_stores_skip_upkeep_without_health_changes() {
    #[derive(Clone, Copy)]
    enum UnavailableStoreKind {
        KeyUnavailable,
        MigrationIncomplete,
    }

    for (suffix, store_kind) in [
        ("key-unavailable", UnavailableStoreKind::KeyUnavailable),
        (
            "migration-incomplete",
            UnavailableStoreKind::MigrationIncomplete,
        ),
    ] {
        let root = tempfile::tempdir().expect("fixture root");
        let state = AsyncSqliteStateStore::open(&root.path().join("state.sqlite"))
            .await
            .expect("fixture state");
        let account_id = AccountId::new(format!("upkeep-store-{suffix}")).expect("account id");
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    suffix,
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await
            .expect("account");
        assert!(
            state
                .claim_credential_refresh(
                    &account_id,
                    codex_router_core::provider::Provider::Openai,
                    codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                    1,
                    2,
                    1_000,
                )
                .await
                .expect("refresh claim")
        );
        assert!(
            state
                .activate_claimed_credential_generation(
                    &account_id,
                    codex_router_core::provider::Provider::Openai,
                    codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                    1,
                    2,
                    1_000,
                )
                .await
                .expect("healthy generation")
        );
        let maintenance_before = state
            .load_credential_maintenance(&account_id)
            .await
            .expect("maintenance query")
            .expect("healthy maintenance");
        let file_store = codex_router_secret_store::file_backend::FileSecretStore::open(
            root.path().join("secrets"),
        )
        .expect("file store");
        let secrets = match store_kind {
            UnavailableStoreKind::KeyUnavailable => {
                EncryptedCredentialStore::key_unavailable(file_store)
            }
            UnavailableStoreKind::MigrationIncomplete => {
                EncryptedCredentialStore::migration_incomplete(
                    file_store,
                    vec![account_id.as_str().to_owned()],
                    codex_router_secret_store::credential_migration::CredentialMigrationFailure::MigrationNotComplete,
                )
            }
        };
        let resolver = codex_router_auth::resolver::AsyncRouterCredentialResolver::new(
            state.clone(),
            secrets.clone(),
            NoopCredentialRefreshClient,
            Some(1_100),
        );
        let resolution = resolver
            .resolve_provider_credentials(
                &account_id,
                codex_router_core::provider::Provider::Openai,
            )
            .await;
        assert_eq!(
            resolution,
            Err(codex_router_auth::resolver::CredentialResolverError::CredentialStoreUnavailable),
            "{suffix}"
        );

        let cycle = run_upkeep_cycle(
            &state,
            &secrets.clone().into(),
            NoopCredentialRefreshClient,
            1_100,
        )
        .await;
        let maintenance_after = state
            .load_credential_maintenance(&account_id)
            .await
            .expect("maintenance query")
            .expect("maintenance remains present");

        assert_eq!(cycle.earliest_due, None, "{suffix}");
        assert!(!cycle.had_local_error, "{suffix}");
        assert_eq!(maintenance_after, maintenance_before, "{suffix}");
        state.close().await.expect("state close");
    }
}

#[derive(Clone)]
struct CountingRefreshClient {
    calls: Arc<AtomicUsize>,
}

impl CredentialRefreshClient for CountingRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(
            codex_router_auth::resolver::CredentialRefreshFailure::ambiguous(
                CredentialFailureClass::ProviderRejected,
            ),
        )
    }
}

#[tokio::test]
async fn terminal_and_cooldown_states_do_not_become_local_worker_failures() {
    let root = std::env::temp_dir().join(format!(
        "router-upkeep-classification-{}-{}",
        std::process::id(),
        crate::credential_upkeep_worker::current_unix_seconds().unwrap_or(0),
    ));
    std::fs::create_dir(&root).expect("fixture root");
    let state = AsyncSqliteStateStore::open(&root.join("state.sqlite"))
        .await
        .expect("fixture state");
    let secrets = codex_router_secret_store::test_support::open_encrypted_credential_store(
        root.join("secrets"),
    )
    .expect("fixture secrets");
    let terminal_id = AccountId::new("upkeep-terminal").expect("terminal id");
    state
        .upsert_account(
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                terminal_id.clone(),
                "terminal",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("terminal account");
    let terminal_key = openai_account_credential_bundle_key(&terminal_id, 1).expect("terminal key");
    secrets
        .write_secret(
            &terminal_key,
            &AccountCredentialBundle::imported_codex_auth("expired-terminal", None)
                .with_expires_unix_seconds(900)
                .to_secret_string()
                .expect("terminal bundle"),
        )
        .expect("terminal secret");
    let terminal_cycle = run_upkeep_cycle(
        &state,
        &secrets.clone().into(),
        NoopCredentialRefreshClient,
        1_000,
    )
    .await;
    assert!(!terminal_cycle.had_local_error);
    assert_eq!(terminal_cycle.earliest_due, None);
    let terminal_health = state
        .load_credential_maintenance(&terminal_id)
        .await
        .expect("terminal health load")
        .expect("terminal health");
    assert_eq!(
        terminal_health.state,
        CredentialMaintenanceState::Unrefreshable
    );

    let cooldown_id = AccountId::new("upkeep-cooldown").expect("cooldown id");
    state
        .upsert_account(
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                cooldown_id.clone(),
                "cooldown",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("cooldown account");
    let cooldown_key = openai_account_credential_bundle_key(&cooldown_id, 1).expect("cooldown key");
    secrets
        .write_secret(
            &cooldown_key,
            &AccountCredentialBundle::imported_codex_auth(
                "expired-cooldown",
                Some("cooldown-refresh".to_owned()),
            )
            .with_expires_unix_seconds(900)
            .to_secret_string()
            .expect("cooldown bundle"),
        )
        .expect("cooldown secret");
    assert!(
        state
            .record_pre_provider_local_failure(&cooldown_id, 1, 1_000)
            .await
            .expect("cooldown state")
    );
    let reauth_id = AccountId::new("upkeep-reauth").expect("reauth id");
    state
        .upsert_account(
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                reauth_id.clone(),
                "reauth",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("reauth account");
    let reauth_key = openai_account_credential_bundle_key(&reauth_id, 1).expect("reauth key");
    secrets
        .write_secret(
            &reauth_key,
            &AccountCredentialBundle::imported_codex_auth(
                "expired-reauth",
                Some("reauth-refresh".to_owned()),
            )
            .with_expires_unix_seconds(900)
            .to_secret_string()
            .expect("reauth bundle"),
        )
        .expect("reauth secret");
    assert!(
        state
            .claim_credential_refresh(
                &reauth_id,
                codex_router_core::provider::Provider::Openai,
                codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                1_000
            )
            .await
            .expect("reauth claim")
    );
    assert!(
        state
            .finish_credential_refresh_claim(
                &reauth_id,
                codex_router_core::provider::Provider::Openai,
                1,
                2,
                codex_router_state::credential_maintenance::CredentialRefreshClaimDisposition::ReauthRequired {
                    failure_class: CredentialFailureClass::ProviderOutcomeAmbiguous,
                },
            )
            .await
            .expect("reauth disposition")
    );
    let cooldown_cycle = run_upkeep_cycle(
        &state,
        &secrets.clone().into(),
        NoopCredentialRefreshClient,
        1_000,
    )
    .await;
    assert!(!cooldown_cycle.had_local_error);
    assert_eq!(cooldown_cycle.earliest_due, Some(1_060));
    let reauth_health = state
        .load_credential_maintenance(&reauth_id)
        .await
        .expect("reauth health load")
        .expect("reauth health");
    assert_eq!(
        reauth_health.state,
        CredentialMaintenanceState::ReauthRequired
    );

    state.close().await.expect("fixture state close");
    let read_only_state = AsyncSqliteStateStore::open_read_only(&root.join("state.sqlite"))
        .await
        .expect("read-only fixture state");
    let overdue_retry = run_upkeep_cycle(
        &read_only_state,
        &secrets.clone().into(),
        NoopCredentialRefreshClient,
        1_060,
    )
    .await;
    assert!(overdue_retry.had_local_error);
    assert_eq!(overdue_retry.earliest_due, Some(1_060));
    assert_eq!(
        bounded_upkeep_wait(overdue_retry, 1_060, Duration::from_secs(200)),
        Duration::from_secs(60)
    );
    read_only_state
        .close()
        .await
        .expect("read-only state close");
    let unavailable_cycle = run_upkeep_cycle(
        &state,
        &secrets.clone().into(),
        NoopCredentialRefreshClient,
        1_000,
    )
    .await;
    assert!(unavailable_cycle.had_local_error);
    assert_eq!(unavailable_cycle.earliest_due, None);
    assert_eq!(
        bounded_upkeep_wait(unavailable_cycle, 1_000, Duration::from_secs(200)),
        Duration::from_secs(60)
    );
    let claim_state = AsyncSqliteStateStore::open(&root.join("state.sqlite"))
        .await
        .expect("claim fixture state");
    for (account_id, label) in [
        (&terminal_id, "terminal"),
        (&cooldown_id, "cooldown"),
        (&reauth_id, "reauth"),
    ] {
        claim_state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    label,
                    AccountStatus::Disabled,
                )
                .with_active_credential_generation(1),
            )
            .await
            .expect("prior fixture account should disable");
    }
    let claimed_id = AccountId::new("upkeep-claimed").expect("claimed id");
    claim_state
        .upsert_account(
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                claimed_id.clone(),
                "claimed",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("claimed account");
    let active_key = openai_account_credential_bundle_key(&claimed_id, 1).expect("active key");
    secrets
        .write_secret(
            &active_key,
            &AccountCredentialBundle::imported_codex_auth(
                "expired-claimed-access",
                Some("claimed-refresh".to_owned()),
            )
            .with_expires_unix_seconds(900)
            .to_secret_string()
            .expect("active bundle"),
        )
        .expect("active secret");
    assert!(
        claim_state
            .claim_credential_refresh(
                &claimed_id,
                codex_router_core::provider::Provider::Openai,
                codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                1_000
            )
            .await
            .expect("unresolved claim")
    );
    let staged_key = openai_account_credential_bundle_key(&claimed_id, 2).expect("staged key");
    secrets
        .write_secret(
            &staged_key,
            &SecretString::new("undecodable-staged-fixture"),
        )
        .expect("undecodable staged secret");
    let calls = Arc::new(AtomicUsize::new(0));
    let claimed_cycle = run_upkeep_cycle(
        &claim_state,
        &secrets.clone().into(),
        CountingRefreshClient {
            calls: Arc::clone(&calls),
        },
        1_000,
    )
    .await;
    assert!(!claimed_cycle.had_local_error);
    assert_eq!(claimed_cycle.earliest_due, None);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let claimed_health = claim_state
        .load_credential_maintenance(&claimed_id)
        .await
        .expect("claim health load")
        .expect("claim health");
    assert_eq!(claimed_health.state, CredentialMaintenanceState::InProgress);
    assert_eq!(
        claim_state
            .load_account(&claimed_id)
            .await
            .expect("claimed account load")
            .expect("claimed account")
            .active_credential_generation(),
        Some(1)
    );
    claim_state.close().await.expect("claim state close");
    drop(secrets);
    std::fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn overdue_or_failed_cycle_never_restarts_with_zero_wait() {
    assert_eq!(
        bounded_upkeep_wait(
            UpkeepCycleResult {
                earliest_due: Some(1),
                had_local_error: false
            },
            0,
            Duration::from_secs(2),
        ),
        Duration::from_secs(1)
    );
    assert_eq!(
        bounded_upkeep_wait(UpkeepCycleResult::default(), 0, Duration::from_secs(10)),
        Duration::from_secs(170)
    );
    assert_eq!(
        bounded_upkeep_wait(
            UpkeepCycleResult {
                earliest_due: None,
                had_local_error: true
            },
            0,
            Duration::from_secs(200),
        ),
        Duration::from_secs(60),
    );
    assert_eq!(
        bounded_upkeep_wait(
            UpkeepCycleResult {
                earliest_due: Some(30),
                had_local_error: true
            },
            0,
            Duration::from_secs(10),
        ),
        Duration::from_secs(20),
    );
    assert_eq!(
        bounded_upkeep_wait(
            UpkeepCycleResult {
                earliest_due: Some(30),
                had_local_error: true,
            },
            0,
            Duration::from_secs(40),
        ),
        Duration::from_secs(60),
    );
}
