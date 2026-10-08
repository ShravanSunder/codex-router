use super::*;
use crate::proxy_role_test_fixtures::*;
use codex_router_auth::resolver::{CredentialRefreshClient, CredentialRefreshFailure};
use codex_router_core::{ids::AccountId, provider::Provider, redaction::SecretString};
use codex_router_keeper_protocol::PrepareMode;
use codex_router_secret_store::{
    SecretStore,
    account_tokens::{AccountCredentialBundle, openai_account_credential_bundle_key},
};
use codex_router_state::{
    account::{AccountRecord, AccountStatus},
    credential_maintenance::CredentialMaintenanceState,
    sqlite::AsyncSqliteStateStore,
};
use std::sync::{Mutex, mpsc};

#[derive(Clone)]
struct HeldRoleRefreshClient {
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}
impl CredentialRefreshClient for HeldRoleRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialRefreshFailure> {
        self.entered
            .send(())
            .expect("fixture claim reaches provider");
        self.release
            .lock()
            .expect("fixture release lock")
            .recv_timeout(Duration::from_secs(90))
            .expect("fixture releases provider");
        Ok(AccountCredentialBundle::imported_codex_auth(
            "role-successor-access",
            Some("role-successor-refresh".to_owned()),
        )
        .with_expires_unix_seconds(10_000))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deactivation_receipt_precedes_a_real_claimed_renewal_completion() {
    let root = tempfile::tempdir().expect("isolated role root");
    drop(
        prepare(root.path(), PrepareMode::Fresh)
            .await
            .expect("secret prerequisites"),
    );
    let state = AsyncSqliteStateStore::open(&root.path().join("state.sqlite"))
        .await
        .expect("actual state owner");
    let account_id = AccountId::new("held-role-renewal").expect("literal account");
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Openai,
                account_id.clone(),
                "held role",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("actual enabled account");
    let secrets = codex_router_secret_store::test_support::open_encrypted_credential_store(
        root.path().join("secrets"),
    )
    .expect("external fixture key");
    secrets
        .write_secret(
            &openai_account_credential_bundle_key(&account_id, 1).expect("active key"),
            &AccountCredentialBundle::imported_codex_auth(
                "role-expired-access",
                Some("role-refresh".to_owned()),
            )
            .with_expires_unix_seconds(900)
            .to_secret_string()
            .expect("fixture bundle"),
        )
        .expect("actual expired credential");
    let (entered, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let (release, held) = mpsc::channel();
    let client = HeldRoleRefreshClient {
        entered,
        release: Arc::new(Mutex::new(held)),
    };
    let prepared = prepare(root.path(), PrepareMode::Fresh)
        .await
        .expect("prepared real role");
    let mut active = prepared.activate_with_worker_starts(
        move |path, credentials, supervisor| async move {
            crate::credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock(
                path, credentials, supervisor, client, ||1_000,
            ).await
        },
        crate::quota::start_background_quota_refresh_worker,
        |_|{}, |_|Ok(()),
    ).await.expect("actual owned producers");
    tokio::time::timeout(Duration::from_secs(2), observed.recv())
        .await
        .expect("bounded provider entry")
        .expect("provider entered");
    let claim = state
        .load_credential_maintenance(&account_id)
        .await
        .expect("real claim read")
        .expect("durable claim");
    let began = std::time::Instant::now();
    let stopped_before_release =
        tokio::time::timeout(Duration::from_millis(100), active.deactivate())
            .await
            .is_ok();
    let stop_elapsed = began.elapsed();
    release.send(()).expect("release only after observing stop");
    active.shutdown().await.expect("join acquired role owners");
    let account = state
        .load_account(&account_id)
        .await
        .expect("successor read")
        .expect("account survives");
    state.close().await.expect("fixture store joins");
    eprintln!(
        "deactivation stop_before_release={stopped_before_release} elapsed_us={} claim={:?} successor={:?}",
        stop_elapsed.as_micros(),
        claim.state,
        account.active_credential_generation()
    );
    assert_eq!(claim.state, CredentialMaintenanceState::InProgress);
    assert_eq!(claim.claimed_successor_generation, Some(2));
    assert_eq!(account.active_credential_generation(), Some(2));
    assert!(
        stopped_before_release,
        "acceptance-stop evidence must precede the held renewal returning"
    );
}
