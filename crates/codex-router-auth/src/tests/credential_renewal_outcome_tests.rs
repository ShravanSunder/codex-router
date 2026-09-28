use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_async_resolvers_use_one_rotating_refresh_generation() {
    let temp_dir = AuthTestTempDir::new("independent-async-refresh");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(FileSecretStore::open(&secret_path));
    let account_id = account_id("independent-async-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "independent",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("original-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let refresh_client = RecordingRefreshClient::new_for_account(
        "independent-async-account",
        "original-refresh-canary",
        AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    )
    .with_delay_millis(100);
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let resolver = AsyncRouterCredentialResolver::new(
            must_ok(AsyncSqliteStateStore::open(&database_path).await),
            secrets.clone(),
            refresh_client.clone(),
            Some(1_000),
        );
        let account_id = account_id.clone();
        tasks.push(tokio::spawn(async move {
            resolver.resolve_provider_credentials(&account_id).await
        }));
    }
    for task in tasks {
        let credential = must_ok(must_ok(task.await));
        assert_eq!(credential.credential_generation(), 2);
    }
    assert_eq!(refresh_client.calls(), 1);
}

#[tokio::test]
async fn orphaned_successor_is_skipped_and_unresolved_claim_blocks_old_token_reuse() {
    let temp_dir = AuthTestTempDir::new("orphaned-successor-claim");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
    let orphan_account_id = account_id("orphaned-successor-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    orphan_account_id.clone(),
                    "orphaned",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    for (generation, access) in [(1, "expired-access-canary"), (2, "orphan-access-canary")] {
        let key = must_ok(account_credential_bundle_key(
            &orphan_account_id,
            generation,
        ));
        must_ok(
            secrets.write_secret(
                &key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        access,
                        Some("old-refresh-canary".to_owned()),
                    )
                    .with_expires_unix_seconds(900)
                    .to_secret_string(),
                ),
            ),
        );
    }
    let refresh_client = RecordingRefreshClient::new_for_account(
        "orphaned-successor-account",
        "old-refresh-canary",
        AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    );
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        refresh_client.clone(),
        Some(1_000),
    );
    let refreshed = must_ok(
        resolver
            .resolve_provider_credentials(&orphan_account_id)
            .await,
    );
    assert_eq!(refreshed.credential_generation(), 3);
    assert_eq!(refresh_client.calls(), 1);

    let blocked_account_id = account_id("crashed-claim-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    blocked_account_id.clone(),
                    "blocked",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let key = must_ok(account_credential_bundle_key(&blocked_account_id, 1));
    must_ok(
        secrets.write_secret(
            &key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "blocked-expired-access-canary",
                    Some("blocked-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    assert!(must_ok(
        state
            .claim_credential_refresh(&blocked_account_id, 1, 3)
            .await
    ));
    let blocked_client = RecordingRefreshClient::new_for_account(
        "crashed-claim-account",
        "blocked-refresh-canary",
        AccountCredentialBundle::imported_codex_auth("should-not-use", None),
    );
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        blocked_client.clone(),
        Some(1_000),
    );
    assert_eq!(
        resolver
            .resolve_provider_credentials(&blocked_account_id)
            .await,
        Err(CredentialResolverError::RefreshUnavailable)
    );
    assert_eq!(blocked_client.calls(), 0);
    let maintenance = must_ok(state.load_credential_maintenance(&blocked_account_id).await)
        .expect("maintenance should persist");
    assert_eq!(maintenance.state.as_str(), "reauth_required");
}

#[tokio::test]
async fn claimed_staged_successor_activates_without_reusing_old_refresh_token() {
    let temp_dir = AuthTestTempDir::new("claimed-staged-successor");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
    let account_id = account_id("claimed-staged-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "staged",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    for (generation, access_token, refresh_token) in [
        (1, "expired-access-canary", "old-refresh-canary"),
        (2, "staged-access-canary", "staged-refresh-canary"),
    ] {
        let key = must_ok(account_credential_bundle_key(&account_id, generation));
        must_ok(
            secrets.write_secret(
                &key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        access_token,
                        Some(refresh_token.to_owned()),
                    )
                    .with_expires_unix_seconds(if generation == 1 { 900 } else { 2_000 })
                    .to_secret_string(),
                ),
            ),
        );
    }
    assert!(must_ok(
        state.claim_credential_refresh(&account_id, 1, 2).await
    ));
    let refresh_client = RecordingRefreshClient::new_for_account(
        "claimed-staged-account",
        "old-refresh-canary",
        AccountCredentialBundle::imported_codex_auth("must-not-use", None),
    );
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        refresh_client.clone(),
        Some(1_000),
    );
    let resolved = must_ok(resolver.resolve_provider_credentials(&account_id).await);
    assert_eq!(resolved.credential_generation(), 2);
    assert_eq!(
        resolved.access_token().expose_secret(),
        "staged-access-canary"
    );
    assert_eq!(refresh_client.calls(), 0);
    let maintenance = must_ok(state.load_credential_maintenance(&account_id).await)
        .expect("maintenance should exist");
    assert_eq!(maintenance.state, CredentialMaintenanceState::Healthy);
}

#[tokio::test]
async fn expired_claimed_successor_is_renewed_before_provider_egress() {
    let temp_dir = AuthTestTempDir::new("expired-staged-successor");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
    let account_id = account_id("expired-staged-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "staged",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    for (generation, access, refresh) in [
        (1, "old-expired-canary", "old-refresh-canary"),
        (2, "staged-expired-canary", "staged-refresh-canary"),
    ] {
        let key = must_ok(account_credential_bundle_key(&account_id, generation));
        must_ok(
            secrets.write_secret(
                &key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(access, Some(refresh.to_owned()))
                        .with_expires_unix_seconds(900)
                        .to_secret_string(),
                ),
            ),
        );
    }
    assert!(must_ok(
        state.claim_credential_refresh(&account_id, 1, 2).await
    ));
    let refresh_client = RecordingRefreshClient::new_for_account(
        "expired-staged-account",
        "staged-refresh-canary",
        AccountCredentialBundle::imported_codex_auth(
            "fresh-access-canary",
            Some("fresh-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    );
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        refresh_client.clone(),
        Some(1_000),
    );

    let resolved = must_ok(resolver.resolve_provider_credentials(&account_id).await);
    assert_eq!(resolved.credential_generation(), 3);
    assert_eq!(
        resolved.access_token().expose_secret(),
        "fresh-access-canary"
    );
    assert_eq!(refresh_client.calls(), 1);
    let active = must_ok(state.load_account(&account_id).await).expect("account");
    assert_eq!(active.active_credential_generation(), Some(3));
}

#[tokio::test]
async fn typed_refresh_failures_persist_retry_or_reauth_without_reusing_ambiguous_token() {
    use crate::resolver::CredentialRefreshFailure;
    use codex_router_state::credential_maintenance::CredentialFailureClass;
    use codex_router_state::credential_maintenance::CredentialMaintenanceState;

    for (name, failure, expected_state, expected_deadline) in [
        (
            "unspent",
            CredentialRefreshFailure::confirmed_unspent(
                CredentialFailureClass::RateLimited,
                Some(90),
            ),
            CredentialMaintenanceState::Retrying,
            Some(1_090),
        ),
        (
            "ambiguous",
            CredentialRefreshFailure::ambiguous(CredentialFailureClass::ProviderRejected),
            CredentialMaintenanceState::ReauthRequired,
            None,
        ),
    ] {
        let temp_dir = AuthTestTempDir::new(name);
        let state =
            must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
        let secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
        let account_id = account_id(name);
        must_ok(
            state
                .upsert_account(
                    &AccountRecord::new(
                        codex_router_core::provider::Provider::Openai,
                        account_id.clone(),
                        name,
                        AccountStatus::Enabled,
                    )
                    .with_active_credential_generation(1),
                )
                .await,
        );
        let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &active_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "expired-access-canary",
                        Some("refresh-token-canary".to_owned()),
                    )
                    .with_expires_unix_seconds(900)
                    .to_secret_string(),
                ),
            ),
        );
        let refresh_client = RejectingRefreshClient {
            failure,
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let resolver = AsyncRouterCredentialResolver::new(
            state.clone(),
            secrets.clone(),
            refresh_client.clone(),
            Some(1_000),
        );
        assert_eq!(
            resolver.resolve_provider_credentials(&account_id).await,
            Err(CredentialResolverError::RefreshUnavailable)
        );
        let maintenance = must_ok(state.load_credential_maintenance(&account_id).await)
            .expect("failure should persist");
        assert_eq!(maintenance.state, expected_state);
        assert_eq!(maintenance.failure_class, Some(failure.failure_class));
        assert_eq!(maintenance.next_attempt_unix_seconds, expected_deadline);
        assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            resolver.resolve_provider_credentials(&account_id).await,
            Err(CredentialResolverError::RefreshUnavailable)
        );
        assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
        if expected_state == CredentialMaintenanceState::Retrying {
            let due_resolver = AsyncRouterCredentialResolver::new(
                state.clone(),
                secrets,
                refresh_client.clone(),
                expected_deadline,
            );
            assert_eq!(
                due_resolver.resolve_provider_credentials(&account_id).await,
                Err(CredentialResolverError::RefreshUnavailable)
            );
            assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 2);
        }
    }
}

#[tokio::test]
async fn elapsed_retry_deadline_renews_even_when_ordinary_renewal_is_not_due() {
    use crate::resolver::CredentialRefreshFailure;
    use codex_router_state::credential_maintenance::CredentialFailureClass;
    use codex_router_state::credential_maintenance::CredentialMaintenanceState;

    let temp_dir = AuthTestTempDir::new("elapsed-retry-deadline");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
    let account_id = account_id("elapsed-retry-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "retry",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    assert!(must_ok(
        state.claim_credential_refresh(&account_id, 1, 2).await
    ));
    let active_key = must_ok(account_credential_bundle_key(&account_id, 2));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "valid-access-canary",
                    Some("refresh-token-canary".to_owned()),
                )
                .with_expires_unix_seconds(10_000)
                .to_secret_string(),
            ),
        ),
    );
    assert!(must_ok(
        state
            .activate_claimed_credential_generation(&account_id, 1, 2, 1_000)
            .await
    ));
    assert!(must_ok(
        state
            .record_pre_provider_local_failure(&account_id, 2, 1_000)
            .await
    ));
    let refresh_client = RejectingRefreshClient {
        failure: CredentialRefreshFailure::confirmed_unspent(
            CredentialFailureClass::RateLimited,
            None,
        ),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        refresh_client.clone(),
        Some(1_060),
    );

    assert_eq!(
        resolver.maintain_account_credentials(&account_id).await,
        Err(CredentialResolverError::RefreshUnavailable)
    );
    assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
    let health = must_ok(state.load_credential_maintenance(&account_id).await).expect("health");
    assert_eq!(health.state, CredentialMaintenanceState::Retrying);
    assert!(
        health
            .next_attempt_unix_seconds
            .is_some_and(|due| due > 1_060)
    );
    assert_eq!(
        must_ok(
            resolver
                .next_maintenance_due_unix_seconds(&account_id)
                .await
        ),
        health.next_attempt_unix_seconds
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_unspent_failure_retries_transient_sqlite_disposition_without_provider_reuse() {
    use crate::resolver::CredentialRefreshFailure;
    use codex_router_state::credential_maintenance::CredentialFailureClass;
    use codex_router_state::credential_maintenance::CredentialMaintenanceState;
    use sqlx::Connection as _;

    let temp_dir = AuthTestTempDir::new("transient-retry-disposition");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
    let account_id = account_id("transient-disposition-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "retry",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("refresh-token-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database_path)
        .create_if_missing(false);
    let mut fault_connection = must_ok(sqlx::SqliteConnection::connect_with(&options).await);
    must_ok(
        sqlx::query(
            "CREATE TRIGGER fail_retry_disposition
         BEFORE UPDATE OF state ON credential_maintenance
         WHEN NEW.state = 'retrying'
         BEGIN SELECT RAISE(ABORT, 'fixture transient write failure'); END;",
        )
        .execute(&mut fault_connection)
        .await,
    );
    must_ok(fault_connection.close().await);
    let release_path = database_path.clone();
    let release_fault = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(release_path)
            .create_if_missing(false);
        let mut connection = sqlx::SqliteConnection::connect_with(&options)
            .await
            .expect("fault database");
        sqlx::query("DROP TRIGGER fail_retry_disposition")
            .execute(&mut connection)
            .await
            .expect("release disposition fault");
        connection.close().await.expect("fault connection close");
    });
    let refresh_client = RejectingRefreshClient {
        failure: CredentialRefreshFailure::confirmed_unspent(
            CredentialFailureClass::RateLimited,
            Some(120),
        ),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        refresh_client.clone(),
        Some(1_000),
    );

    let result = must_ok(
        tokio::time::timeout(
            Duration::from_secs(3),
            resolver.resolve_provider_credentials(&account_id),
        )
        .await,
    );
    assert!(result.is_err());
    must_ok(release_fault.await);
    let health = must_ok(state.load_credential_maintenance(&account_id).await).expect("health");
    assert_eq!(health.state, CredentialMaintenanceState::Retrying);
    assert_eq!(
        health.failure_class,
        Some(CredentialFailureClass::RateLimited)
    );
    assert_eq!(health.next_attempt_unix_seconds, Some(1_120));
    assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
}

#[derive(Clone)]
struct RejectingRefreshClient {
    failure: crate::resolver::CredentialRefreshFailure,
    calls: Arc<AtomicUsize>,
}

impl CredentialRefreshClient for RejectingRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, crate::resolver::CredentialRefreshFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.failure)
    }
}
