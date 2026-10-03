use super::*;

#[tokio::test]
async fn credential_refresh_claim_survives_reopen_and_activates_only_its_reserved_slot() {
    let temp_dir = TestTempDir::new("credential_claim_reopen");
    let database_path = temp_dir.path().join("state.sqlite");
    let account_id = account_id("claimed-account");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    store
        .upsert_account(
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                "claim",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("account should save");
    assert!(
        store
            .claim_credential_refresh(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                1,
                3,
                10
            )
            .await
            .expect("claim should save")
    );
    store.close().await.expect("state should close");

    let reopened = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should reopen");
    let claim = reopened
        .load_credential_maintenance(&account_id)
        .await
        .expect("claim should read")
        .expect("claim should persist");
    assert_eq!(claim.state, CredentialMaintenanceState::InProgress);
    assert_eq!(claim.claimed_successor_generation, Some(3));
    assert!(
        !reopened
            .claim_credential_refresh(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                1,
                4,
                20
            )
            .await
            .expect("second claim should evaluate")
    );
    assert!(
        !reopened
            .activate_claimed_credential_generation(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                100
            )
            .await
            .expect("wrong slot should evaluate")
    );
    assert!(
        reopened
            .activate_claimed_credential_generation(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                1,
                3,
                100
            )
            .await
            .expect("claimed slot should activate")
    );
    let active = reopened
        .load_account(&account_id)
        .await
        .expect("account should load")
        .expect("account should exist");
    assert_eq!(active.active_credential_generation(), Some(3));
    let health = reopened
        .load_credential_maintenance(&account_id)
        .await
        .expect("health should load")
        .expect("health should exist");
    assert_eq!(health.state, CredentialMaintenanceState::Healthy);
    assert_eq!(health.last_success_unix_seconds, Some(100));
}

#[tokio::test]
async fn stale_login_claim_restores_previous_health_instead_of_requiring_login() {
    let temp_dir = TestTempDir::new("credential_stale_login_claim");
    let database_path = temp_dir.path().join("state.sqlite");
    let account_id = account_id("stale-login-claim");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    store
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                "pooled claude",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("account should save");

    assert!(
        store
            .claim_credential_refresh(
                &account_id,
                Provider::Claude,
                ClaimPurpose::Refresh,
                1,
                2,
                900,
            )
            .await
            .expect("initial refresh claim should save")
    );
    assert!(
        store
            .activate_claimed_credential_generation(
                &account_id,
                Provider::Claude,
                ClaimPurpose::Refresh,
                1,
                2,
                1_000,
            )
            .await
            .expect("refresh claim should activate")
    );
    assert!(
        store
            .claim_credential_refresh(
                &account_id,
                Provider::Claude,
                ClaimPurpose::Login,
                2,
                3,
                2_000,
            )
            .await
            .expect("login claim should save")
    );

    let in_progress = store
        .load_credential_maintenance(&account_id)
        .await
        .expect("maintenance should load")
        .expect("login claim should be durable");
    assert_eq!(in_progress.claim_purpose, Some(ClaimPurpose::Login));
    assert_eq!(in_progress.claim_started_unix_seconds, Some(2_000));
    assert_eq!(
        in_progress.claim_prior_state,
        Some(CredentialMaintenanceState::Healthy)
    );

    assert!(
        !store
            .release_stale_login_credential_claim(&account_id, Provider::Claude, 2_300, 300)
            .await
            .expect("claim younger than timeout should remain active")
    );
    assert!(
        store
            .release_stale_login_credential_claim(&account_id, Provider::Claude, 2_301, 300)
            .await
            .expect("stale login claim should be released")
    );

    let restored = store
        .load_credential_maintenance(&account_id)
        .await
        .expect("maintenance should load")
        .expect("pre-login maintenance should be restored");
    assert_eq!(restored.credential_generation, 2);
    assert_eq!(restored.state, CredentialMaintenanceState::Healthy);
    assert_eq!(restored.claimed_successor_generation, None);
    assert_eq!(restored.claim_purpose, None);
    assert_eq!(restored.claim_started_unix_seconds, None);
    assert_eq!(restored.claim_prior_state, None);
}

#[tokio::test]
async fn login_claim_accepts_a_new_disabled_account_and_removes_its_claim_on_activation() {
    let temp_dir = TestTempDir::new("credential_login_claim_first_generation");
    let database_path = temp_dir.path().join("state.sqlite");
    let account_id = account_id("login-claim-first-generation");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    store
        .upsert_account(&AccountRecord::new(
            Provider::Claude,
            account_id.clone(),
            "first login",
            AccountStatus::Disabled,
        ))
        .await
        .expect("new account metadata should save");

    assert!(
        store
            .claim_credential_refresh(
                &account_id,
                Provider::Claude,
                ClaimPurpose::Login,
                0,
                1,
                100,
            )
            .await
            .expect("login claim should evaluate")
    );
    assert!(
        store
            .activate_claimed_credential_generation(
                &account_id,
                Provider::Claude,
                ClaimPurpose::Login,
                0,
                1,
                100,
            )
            .await
            .expect("login claim should activate")
    );

    let active_account = store
        .load_account(&account_id)
        .await
        .expect("account should load")
        .expect("account should exist");
    assert_eq!(active_account.provider(), Provider::Claude);
    assert_eq!(active_account.status(), AccountStatus::Enabled);
    assert_eq!(active_account.active_credential_generation(), Some(1));
    assert!(
        store
            .load_credential_maintenance(&account_id)
            .await
            .expect("maintenance should load")
            .is_none()
    );
}

#[tokio::test]
async fn generation_claim_rejects_a_provider_mismatch_for_both_purposes() {
    let temp_dir = TestTempDir::new("credential_claim_provider_guard");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    let claude_account_id = account_id("claim-provider-claude");
    store
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                claude_account_id.clone(),
                "claude",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("Claude account should save");
    let openai_account_id = account_id("claim-provider-openai");
    store
        .upsert_account(
            &AccountRecord::new(
                Provider::Openai,
                openai_account_id.clone(),
                "openai",
                AccountStatus::Disabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("OpenAI account should save");

    assert!(
        !store
            .claim_credential_refresh(
                &claude_account_id,
                Provider::Openai,
                ClaimPurpose::Refresh,
                1,
                2,
                100,
            )
            .await
            .expect("refresh provider guard should evaluate")
    );
    assert!(
        !store
            .claim_credential_refresh(
                &openai_account_id,
                Provider::Claude,
                ClaimPurpose::Login,
                1,
                2,
                100,
            )
            .await
            .expect("login provider guard should evaluate")
    );
}

#[tokio::test]
async fn current_generation_claim_replaces_stale_maintenance_without_reviving_old_claim() {
    let temp_dir = TestTempDir::new("credential_claim_new_generation");
    let database_path = temp_dir.path().join("state.sqlite");
    let account_id = account_id("replaced-credential-account");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    store
        .upsert_account(
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                "replaced",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("old account should save");
    assert!(
        store
            .claim_credential_refresh(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                100
            )
            .await
            .expect("old claim")
    );
    store
        .upsert_account(
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                "replaced",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(3),
        )
        .await
        .expect("new login should save");

    assert!(
        store
            .claim_credential_refresh(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                3,
                4,
                200
            )
            .await
            .expect("new claim")
    );
    assert!(
        !store
            .activate_claimed_credential_generation(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                100
            )
            .await
            .expect("old claim cannot activate")
    );
    let claim = store
        .load_credential_maintenance(&account_id)
        .await
        .expect("claim load")
        .expect("claim exists");
    assert_eq!(claim.credential_generation, 3);
    assert_eq!(claim.claimed_successor_generation, Some(4));
    assert!(
        store
            .activate_claimed_credential_generation(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                3,
                4,
                100
            )
            .await
            .expect("new claim activates")
    );
}

#[tokio::test]
async fn maintenance_success_time_survives_current_generation_failures_only() {
    use crate::credential_maintenance::CredentialFailureClass;

    let temp_dir = TestTempDir::new("maintenance_success_generation");
    let state = AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite"))
        .await
        .expect("state should open");
    let account_id = account_id("success-time-account");
    let account = |generation| {
        AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "success time",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(generation)
    };
    state
        .upsert_account(&account(1))
        .await
        .expect("initial account");
    assert!(
        state
            .claim_credential_refresh(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                100
            )
            .await
            .expect("initial claim")
    );
    assert!(
        state
            .activate_claimed_credential_generation(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                1,
                2,
                100
            )
            .await
            .expect("initial activation")
    );

    assert!(
        state
            .claim_credential_refresh(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                2,
                3,
                200
            )
            .await
            .expect("same-generation claim")
    );
    let current_claim = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("claim load")
        .expect("claim");
    assert_eq!(current_claim.last_success_unix_seconds, Some(100));
    assert!(
        state
            .finish_credential_refresh_claim(
                &account_id,
                Provider::Openai,
                2,
                3,
                CredentialRefreshClaimDisposition::Retrying {
                    failure_class: CredentialFailureClass::TransportUnspent,
                    next_attempt_unix_seconds: 200,
                },
            )
            .await
            .expect("safe claim failure")
    );
    assert!(
        state
            .record_pre_provider_local_failure(&account_id, 2, 200)
            .await
            .expect("same-generation local failure")
    );
    let current_failure = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("failure load")
        .expect("failure");
    assert_eq!(current_failure.last_success_unix_seconds, Some(100));

    state
        .upsert_account(&account(4))
        .await
        .expect("replacement account");
    assert!(
        state
            .record_pre_provider_local_failure(&account_id, 4, 300)
            .await
            .expect("replacement local failure")
    );
    let replacement_failure = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("replacement failure load")
        .expect("replacement failure");
    assert_eq!(replacement_failure.credential_generation, 4);
    assert_eq!(replacement_failure.last_success_unix_seconds, None);

    state
        .upsert_account(&account(6))
        .await
        .expect("second replacement account");
    assert!(
        state
            .claim_credential_refresh(
                &account_id,
                Provider::Openai,
                crate::credential_maintenance::ClaimPurpose::Refresh,
                6,
                7,
                400
            )
            .await
            .expect("replacement claim")
    );
    let replacement_claim = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("replacement claim load")
        .expect("replacement claim");
    assert_eq!(replacement_claim.credential_generation, 6);
    assert_eq!(replacement_claim.last_success_unix_seconds, None);
}
