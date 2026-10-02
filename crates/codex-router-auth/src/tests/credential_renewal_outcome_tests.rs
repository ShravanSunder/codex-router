use super::*;
use crate::resolver::CredentialRefreshFailure;
use codex_router_core::provider::Provider;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::provider_credential_bundle_key;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::model::SecretKey;
use codex_router_secret_store::model::SecretStoreError;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn claude_providerless_refresh_is_confirmed_unspent_without_maintenance_write() {
    let temp_dir = AuthTestTempDir::new("claude-providerless-refresh");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let account_id = account_id("claude-providerless-refresh");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "Claude providerless refresh",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    assert!(must_ok(
        state
            .record_pre_provider_local_failure(&account_id, 1, 1_000)
            .await
    ));
    let maintenance_before = must_ok(state.load_credential_maintenance(&account_id).await)
        .expect("the fixture should have a maintenance row");

    let refresh_client = crate::claude_oauth::ClaudeOAuthRefreshClient::new();
    let failure = CredentialRefreshClient::refresh_credentials(
        &refresh_client,
        &account_id,
        &SecretString::new("unused-providerless-refresh-canary"),
    );

    assert_eq!(
        failure,
        Err(CredentialRefreshFailure::confirmed_unspent(
            codex_router_state::credential_maintenance::CredentialFailureClass::LocalPersistence,
            None,
        ))
    );
    assert_eq!(
        must_ok(state.load_credential_maintenance(&account_id).await),
        Some(maintenance_before),
        "the provider-less refusal must not write credential maintenance"
    );
}

#[derive(Clone)]
struct ProviderPruningRefreshClient;

impl CredentialRefreshClient for ProviderPruningRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialRefreshFailure> {
        Err(CredentialRefreshFailure::ambiguous(
            codex_router_state::credential_maintenance::CredentialFailureClass::ProviderOutcomeAmbiguous,
        ))
    }

    fn refresh_provider_credentials(
        &self,
        provider: codex_router_core::provider::Provider,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<CredentialBundle, CredentialRefreshFailure> {
        match provider {
            codex_router_core::provider::Provider::Openai => Ok(CredentialBundle::OpenAi(
                AccountCredentialBundle::imported_codex_auth(
                    "pruning-refreshed-openai-access",
                    Some("pruning-refreshed-openai-refresh".to_owned()),
                )
                .with_expires_unix_seconds(2_000_000),
            )),
            codex_router_core::provider::Provider::Claude => {
                CredentialBundle::new_claude(
                    SecretString::new("pruning-refreshed-claude-access"),
                    SecretString::new("pruning-refreshed-claude-refresh"),
                    2_000_000,
                )
                .map_err(|_| {
                    CredentialRefreshFailure::ambiguous(
                        codex_router_state::credential_maintenance::CredentialFailureClass::MalformedResponse,
                    )
                })
            }
        }
    }
}

#[derive(Clone)]
struct ClaimReleasingSecretStore {
    inner: EncryptedCredentialStore,
    database_path: PathBuf,
    account_id: AccountId,
    claim_released: Arc<AtomicBool>,
}

impl SecretStore for ClaimReleasingSecretStore {
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        self.inner.write_secret(key, secret)
    }

    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        self.inner.read_secret(key)
    }

    fn delete_staged(&self, key: &SecretKey) -> Result<(), SecretStoreError> {
        self.inner.delete_staged(key)
    }

    fn write_staged(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        self.inner.write_staged(key, secret)?;
        if !self.claim_released.swap(true, Ordering::SeqCst) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| SecretStoreError::KeyUnavailable)?;
            let database_path = self.database_path.clone();
            let account_id = self.account_id.clone();
            runtime.block_on(async move {
                let state = AsyncSqliteStateStore::open(&database_path)
                    .await
                    .map_err(|_| SecretStoreError::KeyUnavailable)?;
                let released = state
                    .finish_credential_refresh_claim(
                        &account_id,
                        Provider::Openai,
                        3,
                        4,
                        codex_router_state::credential_maintenance::CredentialRefreshClaimDisposition::ReauthRequired {
                            failure_class: codex_router_state::credential_maintenance::CredentialFailureClass::ProviderOutcomeAmbiguous,
                        },
                    )
                    .await
                    .map_err(|_| SecretStoreError::KeyUnavailable)?;
                state
                    .close()
                    .await
                    .map_err(|_| SecretStoreError::KeyUnavailable)?;
                if !released {
                    return Err(SecretStoreError::KeyUnavailable);
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    fn prune_obsolete_generations(
        &self,
        provider: Provider,
        account_id: &AccountId,
        active_generation: u64,
    ) -> Result<Vec<u64>, SecretStoreError> {
        self.inner
            .prune_obsolete_generations(provider, account_id, active_generation)
    }
}

#[tokio::test]
async fn committed_provider_refresh_keeps_active_and_previous_generation_only() {
    use codex_router_secret_store::account_tokens::provider_credential_bundle_key;

    let temp_dir = AuthTestTempDir::new("provider-refresh-generation-pruning");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let secret_root = temp_dir.path().join("secrets");
    let (secrets, read_trace, write_trace) = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store_with_read_write_traces(
            &secret_root,
        ),
    );
    let openai_id = account_id("prune-openai-account");
    let claude_id = account_id("prune-claude-account");
    let accounts = [
        (Provider::Openai, &openai_id),
        (Provider::Claude, &claude_id),
    ];

    for (provider, account_id) in accounts {
        must_ok(
            state
                .upsert_account(
                    &AccountRecord::new(
                        provider,
                        account_id.clone(),
                        "generation pruning",
                        AccountStatus::Enabled,
                    )
                    .with_active_credential_generation(3),
                )
                .await,
        );
        for generation in 1..=3 {
            let active_provider_key = must_ok(provider_credential_bundle_key(
                provider, account_id, generation,
            ));
            let active_provider_bundle = test_pruning_bundle(provider, generation);
            must_ok(secrets.write_secret(
                &active_provider_key,
                &must_ok(active_provider_bundle.to_secret_string()),
            ));

            let other_provider = match provider {
                Provider::Openai => Provider::Claude,
                Provider::Claude => Provider::Openai,
            };
            if generation <= 2 {
                let other_provider_key = must_ok(provider_credential_bundle_key(
                    other_provider,
                    account_id,
                    generation,
                ));
                let other_provider_bundle = test_pruning_bundle(other_provider, generation);
                must_ok(secrets.write_secret(
                    &other_provider_key,
                    &must_ok(other_provider_bundle.to_secret_string()),
                ));
            }
        }
    }

    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        ProviderPruningRefreshClient,
        Some(1_000),
    );
    for (_, account_id) in accounts {
        must_ok(resolver.maintain_account_credentials(account_id).await);
        assert_eq!(
            must_ok(state.load_account(account_id).await)
                .expect("refreshed provider account remains")
                .active_credential_generation(),
            Some(4)
        );
    }

    let read_events = read_trace.events();
    let home_root = std::path::PathBuf::from(
        std::env::var_os("HOME").expect("HOME should be available for native-path proof"),
    );
    let native_auth_store_roots = [home_root.join(".codex"), home_root.join(".claude")];
    assert!(
        read_events.iter().any(|event| matches!(
            event,
            codex_router_secret_store::test_support::FileReadTraceEvent::FileOpenAttempted { path }
                if path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(".v2"))
        )),
        "refresh must trace encrypted credential reads"
    );
    assert!(
        read_events.iter().any(|event| matches!(
            event,
            codex_router_secret_store::test_support::FileReadTraceEvent::DirectoryOpenAttempted { path }
                if path == &secret_root
        )),
        "refresh generation pruning must trace its encrypted-store directory read"
    );
    for event in read_events {
        let path = match event {
            codex_router_secret_store::test_support::FileReadTraceEvent::FileOpenAttempted {
                path,
            }
            | codex_router_secret_store::test_support::FileReadTraceEvent::DirectoryOpenAttempted {
                path,
            } => path,
        };
        assert!(
            path.starts_with(&secret_root),
            "refresh opened a path outside the Router secret store: {path:?}"
        );
        assert!(
            native_auth_store_roots
                .iter()
                .all(|native_root| !path.starts_with(native_root)),
            "refresh opened a path under a native auth store: {path:?}"
        );
    }

    for (provider, account_id) in accounts {
        let other_provider = match provider {
            Provider::Openai => Provider::Claude,
            Provider::Claude => Provider::Openai,
        };
        for generation in 1..=2 {
            let active_provider_key = must_ok(provider_credential_bundle_key(
                provider, account_id, generation,
            ));
            let other_provider_key = must_ok(provider_credential_bundle_key(
                other_provider,
                account_id,
                generation,
            ));
            assert!(
                secrets.read_secret(&active_provider_key).is_err(),
                "generations older than active - 1 are pruned for {provider:?}"
            );
            assert!(
                secrets.read_secret(&other_provider_key).is_ok(),
                "pruning {provider:?} must leave {other_provider:?} generations intact"
            );
        }
        for generation in 3..=4 {
            let key = must_ok(provider_credential_bundle_key(
                provider, account_id, generation,
            ));
            assert!(secrets.read_secret(&key).is_ok());
        }
    }

    let token_canaries = [
        "openai-pruning-access-1",
        "openai-pruning-refresh-1",
        "openai-pruning-access-2",
        "openai-pruning-refresh-2",
        "openai-pruning-access-3",
        "openai-pruning-refresh-3",
        "claude-pruning-access-1",
        "claude-pruning-refresh-1",
        "claude-pruning-access-2",
        "claude-pruning-refresh-2",
        "claude-pruning-access-3",
        "claude-pruning-refresh-3",
        "pruning-refreshed-openai-access",
        "pruning-refreshed-openai-refresh",
        "pruning-refreshed-claude-access",
        "pruning-refreshed-claude-refresh",
    ];
    for event in write_trace.events() {
        if let codex_router_secret_store::test_support::FileWriteTraceEvent::TemporaryFileWritten {
            contents,
            ..
        } = event
        {
            let temporary_bytes = String::from_utf8_lossy(&contents);
            for canary in token_canaries {
                assert!(!temporary_bytes.contains(canary));
            }
        }
    }
    for file in must_ok(std::fs::read_dir(&secret_root)) {
        let path = must_ok(file).path();
        let file_bytes = must_ok(std::fs::read(path));
        let stored_file_contents = String::from_utf8_lossy(&file_bytes);
        for canary in token_canaries {
            assert!(!stored_file_contents.contains(canary));
        }
    }
}

#[tokio::test]
async fn ambiguous_refresh_activation_does_not_prune_any_generation() {
    use codex_router_secret_store::account_tokens::provider_credential_bundle_key;

    let temp_dir = AuthTestTempDir::new("ambiguous-refresh-activation-pruning");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("ambiguous-prune-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Openai,
                    account_id.clone(),
                    "ambiguous activation",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(3),
            )
            .await,
    );
    for generation in 1..=3 {
        let key = must_ok(provider_credential_bundle_key(
            Provider::Openai,
            &account_id,
            generation,
        ));
        let bundle = AccountCredentialBundle::imported_codex_auth(
            format!("ambiguous-access-{generation}"),
            Some(format!("ambiguous-refresh-{generation}")),
        )
        .with_expires_unix_seconds(900);
        must_ok(secrets.write_secret(&key, &must_ok(bundle.to_secret_string())));
    }
    let wrapped_secrets = ClaimReleasingSecretStore {
        inner: secrets.clone(),
        database_path,
        account_id: account_id.clone(),
        claim_released: Arc::new(AtomicBool::new(false)),
    };
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        wrapped_secrets,
        ProviderPruningRefreshClient,
        Some(1_000),
    );

    assert_eq!(
        resolver.maintain_account_credentials(&account_id).await,
        Err(CredentialResolverError::RefreshUnavailable)
    );
    assert_eq!(
        must_ok(state.load_account(&account_id).await)
            .expect("account should remain registered")
            .active_credential_generation(),
        Some(3)
    );
    for generation in 1..=4 {
        let key = must_ok(provider_credential_bundle_key(
            Provider::Openai,
            &account_id,
            generation,
        ));
        assert!(
            secrets.read_secret(&key).is_ok(),
            "ambiguous activation leaves generation {generation} untouched"
        );
    }
    assert!(must_ok(state.load_credential_maintenance(&account_id).await)
        .is_some_and(|record| record.state == codex_router_state::credential_maintenance::CredentialMaintenanceState::ReauthRequired));
}

fn test_pruning_bundle(
    provider: codex_router_core::provider::Provider,
    generation: u64,
) -> CredentialBundle {
    match provider {
        codex_router_core::provider::Provider::Openai => CredentialBundle::OpenAi(
            AccountCredentialBundle::imported_codex_auth(
                format!("openai-pruning-access-{generation}"),
                Some(format!("openai-pruning-refresh-{generation}")),
            )
            .with_expires_unix_seconds(10_000_000),
        ),
        codex_router_core::provider::Provider::Claude => must_ok(CredentialBundle::new_claude(
            SecretString::new(format!("claude-pruning-access-{generation}")),
            SecretString::new(format!("claude-pruning-refresh-{generation}")),
            10_000_000,
        )),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_async_resolvers_use_one_rotating_refresh_generation() {
    let temp_dir = AuthTestTempDir::new("independent-async-refresh");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path),
    );
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
    let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
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
            resolver
                .resolve_provider_credentials(&account_id, Provider::Openai)
                .await
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
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
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
        let key = must_ok(openai_account_credential_bundle_key(
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
            .resolve_provider_credentials(&orphan_account_id, Provider::Openai)
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
    let key = must_ok(openai_account_credential_bundle_key(&blocked_account_id, 1));
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
            .claim_credential_refresh(
                &blocked_account_id,
                codex_router_core::provider::Provider::Openai,
                codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                1,
                3,
                1_000
            )
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
            .resolve_provider_credentials(&blocked_account_id, Provider::Openai)
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
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
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
        let key = must_ok(openai_account_credential_bundle_key(
            &account_id,
            generation,
        ));
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
        state
            .claim_credential_refresh(
                &account_id,
                codex_router_core::provider::Provider::Openai,
                codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                1_000
            )
            .await
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
    let resolved = must_ok(
        resolver
            .resolve_provider_credentials(&account_id, Provider::Openai)
            .await,
    );
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
async fn orphaned_login_claim_with_staged_bundle_activates_without_refresh() {
    let temp_dir = AuthTestTempDir::new("orphaned-login-claim-with-staged-bundle");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("orphaned-login-with-stage");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "Claude staged login",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    for (generation, access_token, expires_at) in [
        (1, "expired-old-access", 900),
        (2, "staged-login-access", 2_000),
    ] {
        let key = must_ok(provider_credential_bundle_key(
            Provider::Claude,
            &account_id,
            generation,
        ));
        let bundle = must_ok(CredentialBundle::new_claude(
            SecretString::new(access_token),
            SecretString::new(format!("{access_token}-refresh")),
            expires_at,
        ));
        must_ok(secrets.write_secret(&key, &must_ok(bundle.to_secret_string())));
    }
    assert!(must_ok(
        state
            .claim_credential_refresh(
                &account_id,
                Provider::Claude,
                codex_router_state::credential_maintenance::ClaimPurpose::Login,
                1,
                2,
                950,
            )
            .await
    ));
    assert!(
        must_ok(state.load_account(&account_id).await).is_some(),
        "account survives login claim creation"
    );

    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        ProviderPruningRefreshClient,
        Some(1_000),
    );
    let resolved = must_ok(
        resolver
            .resolve_provider_credentials(&account_id, Provider::Claude)
            .await,
    );

    assert_eq!(resolved.credential_generation(), 2);
    assert_eq!(
        resolved.access_token().expose_secret(),
        "staged-login-access"
    );
    let account =
        must_ok(state.load_account(&account_id).await).expect("account remains registered");
    assert_eq!(account.active_credential_generation(), Some(2));
    assert_eq!(
        must_ok(state.load_credential_maintenance(&account_id).await),
        None,
        "a login claim without prior maintenance leaves no transient row"
    );
}

#[tokio::test]
async fn orphaned_login_claim_without_staged_bundle_restores_prior_maintenance() {
    use codex_router_state::credential_maintenance::CredentialRefreshClaimDisposition;

    let temp_dir = AuthTestTempDir::new("orphaned-login-claim-without-staged-bundle");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("orphaned-login-without-stage");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "Claude interrupted login",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(provider_credential_bundle_key(
        Provider::Claude,
        &account_id,
        1,
    ));
    let active_bundle = must_ok(CredentialBundle::new_claude(
        SecretString::new("expired-old-access"),
        SecretString::new("old-refresh-token"),
        900,
    ));
    must_ok(secrets.write_secret(&active_key, &must_ok(active_bundle.to_secret_string())));
    assert!(must_ok(
        state
            .claim_credential_refresh(
                &account_id,
                Provider::Claude,
                codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                700,
            )
            .await
    ));
    assert!(must_ok(
        state
            .finish_credential_refresh_claim(
                &account_id,
                Provider::Claude,
                1,
                2,
                CredentialRefreshClaimDisposition::Retrying {
                    failure_class:
                        codex_router_state::credential_maintenance::CredentialFailureClass::TransportUnspent,
                    next_attempt_unix_seconds: 2_000,
                },
            )
            .await
    ));
    let prior_maintenance = must_ok(state.load_credential_maintenance(&account_id).await)
        .expect("retrying maintenance should be recorded");
    assert!(must_ok(
        state
            .claim_credential_refresh(
                &account_id,
                Provider::Claude,
                codex_router_state::credential_maintenance::ClaimPurpose::Login,
                1,
                3,
                1_000,
            )
            .await
    ));
    assert!(
        must_ok(state.load_account(&account_id).await).is_some(),
        "account survives orphaned login claim creation"
    );
    let in_progress = must_ok(state.load_credential_maintenance(&account_id).await)
        .expect("login claim with prior retry state should decode");
    assert_eq!(
        in_progress.claim_prior_state,
        Some(CredentialMaintenanceState::Retrying)
    );
    assert_eq!(in_progress.next_attempt_unix_seconds, Some(2_000));

    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        ProviderPruningRefreshClient,
        Some(1_301),
    );
    let resolution = resolver
        .resolve_provider_credentials(&account_id, Provider::Claude)
        .await;
    let restored_maintenance = must_ok(state.load_credential_maintenance(&account_id).await)
        .expect("prior maintenance should be restored");
    assert_eq!(restored_maintenance, prior_maintenance);
    assert_eq!(resolution, Err(CredentialResolverError::RefreshUnavailable));
}

#[tokio::test]
async fn expired_claimed_successor_is_renewed_before_provider_egress() {
    let temp_dir = AuthTestTempDir::new("expired-staged-successor");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
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
        let key = must_ok(openai_account_credential_bundle_key(
            &account_id,
            generation,
        ));
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
        state
            .claim_credential_refresh(
                &account_id,
                codex_router_core::provider::Provider::Openai,
                codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                1_000
            )
            .await
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

    let resolved = must_ok(
        resolver
            .resolve_provider_credentials(&account_id, Provider::Openai)
            .await,
    );
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
        let secrets = must_ok(
            codex_router_secret_store::test_support::open_encrypted_credential_store(
                temp_dir.path().join("secrets"),
            ),
        );
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
        let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
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
            resolver
                .resolve_provider_credentials(&account_id, Provider::Openai)
                .await,
            Err(CredentialResolverError::RefreshUnavailable)
        );
        let maintenance = must_ok(state.load_credential_maintenance(&account_id).await)
            .expect("failure should persist");
        assert_eq!(maintenance.state, expected_state);
        assert_eq!(maintenance.failure_class, Some(failure.failure_class));
        assert_eq!(maintenance.next_attempt_unix_seconds, expected_deadline);
        assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            resolver
                .resolve_provider_credentials(&account_id, Provider::Openai)
                .await,
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
                due_resolver
                    .resolve_provider_credentials(&account_id, Provider::Openai)
                    .await,
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
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
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
        state
            .claim_credential_refresh(
                &account_id,
                codex_router_core::provider::Provider::Openai,
                codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                1_000
            )
            .await
    ));
    let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 2));
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
            .activate_claimed_credential_generation(
                &account_id,
                codex_router_core::provider::Provider::Openai,
                codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                1_000
            )
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
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
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
    let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
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
            resolver.resolve_provider_credentials(&account_id, Provider::Openai),
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
