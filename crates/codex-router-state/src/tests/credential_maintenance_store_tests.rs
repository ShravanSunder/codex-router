use super::*;
use crate::credential_maintenance::CredentialFailureClass;

#[tokio::test]
async fn rejected_active_generation_becomes_reauth_required() {
    let temp_dir = TestTempDir::new("rejected_active_generation_reauth");
    let state = AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite"))
        .await
        .expect("state should open");
    let account_id = account_id("rejected-active-generation-reauth");
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                "rejected active generation",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("account should save");

    assert!(
        state
            .mark_generation_reauth_required(&account_id, 1)
            .await
            .expect("active rejected generation should be marked")
    );
    let maintenance = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("maintenance should load")
        .expect("reauth state should be persisted");

    assert_eq!(maintenance.credential_generation, 1);
    assert_eq!(
        maintenance.state,
        CredentialMaintenanceState::ReauthRequired
    );
    assert_eq!(
        maintenance.failure_class,
        Some(CredentialFailureClass::ProviderRejected)
    );
}

#[tokio::test]
async fn rejected_new_generation_resets_older_generation_maintenance() {
    let temp_dir = TestTempDir::new("rejected_new_generation_resets_maintenance");
    let state = AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite"))
        .await
        .expect("state should open");
    let account_id = account_id("rejected-new-generation-resets-maintenance");
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                "rejected newer generation",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("account should save");

    assert!(
        state
            .claim_credential_refresh(
                &account_id,
                Provider::Claude,
                ClaimPurpose::Refresh,
                1,
                2,
                900,
            )
            .await
            .expect("generation 2 refresh claim should persist")
    );
    assert!(
        state
            .activate_claimed_credential_generation(
                &account_id,
                Provider::Claude,
                ClaimPurpose::Refresh,
                1,
                2,
                1_000,
            )
            .await
            .expect("generation 2 refresh should activate")
    );
    assert!(
        state
            .record_pre_provider_local_failure(&account_id, 2, 1_200)
            .await
            .expect("generation 2 local failure should persist")
    );
    let older_maintenance = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("older maintenance should load")
        .expect("generation 2 maintenance should be present");
    assert_eq!(older_maintenance.credential_generation, 2);
    assert_eq!(older_maintenance.last_success_unix_seconds, Some(1_000));
    assert_eq!(
        older_maintenance.failure_class,
        Some(CredentialFailureClass::LocalPersistence)
    );
    assert_eq!(older_maintenance.next_attempt_unix_seconds, Some(1_260));
    assert_eq!(older_maintenance.consecutive_failures, 1);

    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                "rejected newer generation",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(3),
        )
        .await
        .expect("account should advance to generation 3");

    assert!(
        state
            .mark_generation_reauth_required(&account_id, 3)
            .await
            .expect("active rejected generation should replace older maintenance")
    );
    let maintenance = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("reauth maintenance should load")
        .expect("reauth maintenance should be present");

    assert_eq!(maintenance.credential_generation, 3);
    assert_eq!(
        maintenance.state,
        CredentialMaintenanceState::ReauthRequired
    );
    assert_eq!(
        maintenance.failure_class,
        Some(CredentialFailureClass::ProviderRejected)
    );
    assert_eq!(maintenance.last_success_unix_seconds, None);
    assert_eq!(maintenance.next_attempt_unix_seconds, None);
    assert_eq!(maintenance.claimed_successor_generation, None);
    assert_eq!(maintenance.claim_purpose, None);
    assert_eq!(maintenance.claim_started_unix_seconds, None);
    assert_eq!(maintenance.claim_prior_state, None);
    assert_eq!(maintenance.consecutive_failures, 0);
}

#[tokio::test]
async fn rejected_older_generation_does_not_change_newer_active_generation_maintenance() {
    let temp_dir = TestTempDir::new("rejected_older_generation_reauth");
    let state = AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite"))
        .await
        .expect("state should open");
    let account_id = account_id("rejected-older-generation-reauth");
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                "newer active generation",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(2),
        )
        .await
        .expect("account should save");
    assert!(
        state
            .record_pre_provider_local_failure(&account_id, 2, 1_000)
            .await
            .expect("current generation maintenance should persist")
    );
    let maintenance_before = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("maintenance should load")
        .expect("retry state should be present");

    assert!(
        !state
            .mark_generation_reauth_required(&account_id, 1)
            .await
            .expect("stale rejected generation should be checked")
    );
    assert_eq!(
        state
            .load_credential_maintenance(&account_id)
            .await
            .expect("maintenance should load after stale rejection"),
        Some(maintenance_before)
    );
}

#[tokio::test]
async fn rejected_generation_does_not_overwrite_in_progress_successor_claim() {
    let temp_dir = TestTempDir::new("rejected_generation_with_claim_reauth");
    let state = AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite"))
        .await
        .expect("state should open");
    let account_id = account_id("rejected-generation-with-claim-reauth");
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                "claimed rejected generation",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("account should save");
    assert!(
        state
            .claim_credential_refresh(
                &account_id,
                Provider::Claude,
                ClaimPurpose::Refresh,
                1,
                2,
                10
            )
            .await
            .expect("successor claim should persist")
    );
    let maintenance_before = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("maintenance should load")
        .expect("successor claim should be present");

    assert!(
        !state
            .mark_generation_reauth_required(&account_id, 1)
            .await
            .expect("rejected generation should defer to in-progress claim")
    );
    assert_eq!(
        state
            .load_credential_maintenance(&account_id)
            .await
            .expect("maintenance should load after rejection"),
        Some(maintenance_before)
    );
}
