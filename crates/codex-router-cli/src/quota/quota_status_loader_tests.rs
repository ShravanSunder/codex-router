use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::keychain_data_key::KeychainAccess;
use codex_router_secret_store::keychain_data_key::KeychainAccessError;
use codex_router_secret_store::keychain_data_key::ROUTER_KEYCHAIN_SERVICE;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::window_observation::WindowObservation;
use codex_router_state::window_observation::WindowObservationProps;
use tempfile::TempDir;

use super::*;
use crate::quota_reset::FixedOriginInteractiveResetSessionFactory;
use crate::quota_reset::InteractiveResetSessionFactory;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus;

#[path = "quota_status_recovery_tests.rs"]
mod recovery_tests;

#[tokio::test]
async fn quota_status_report_uses_provider_refresh_status_and_preserves_claude_windows() {
    use codex_router_core::route_profile::WindowKind;

    let test_root = TempDir::new().expect("quota status root");
    let router_root = test_root.path().join("router");
    std::fs::create_dir_all(&router_root).expect("router root");
    let state = AsyncSqliteStateStore::open(&router_root.join("state.sqlite"))
        .await
        .expect("state store should open");
    let claude_account_id = AccountId::new("claude_refresh_status").expect("Claude account id");
    let openai_account_id = AccountId::new("openai_refresh_status").expect("OpenAI account id");
    for (provider, account_id, label) in [
        (Provider::Claude, &claude_account_id, "claude-status"),
        (Provider::Openai, &openai_account_id, "openai-status"),
    ] {
        state
            .upsert_account(
                &AccountRecord::new(provider, account_id.clone(), label, AccountStatus::Enabled)
                    .with_active_credential_generation(1),
            )
            .await
            .expect("account should persist");
    }
    let claude_observations = [
        (WindowKind::FiveHour, 6_000, 19_000),
        (WindowKind::Weekly, 7_000, 605_000),
    ]
    .map(
        |(window_kind, remaining_basis_points, reset_unix_seconds)| {
            WindowObservation::new(
                WindowObservationProps::new(
                    claude_account_id.clone(),
                    window_kind,
                    remaining_basis_points,
                    900,
                )
                .with_reset_unix_seconds(reset_unix_seconds)
                .with_fresh_until_unix_seconds(1_500),
            )
            .expect("last-good Claude observation should validate")
        },
    );
    for observation in &claude_observations {
        state
            .record_window_observation(observation, || 900)
            .await
            .expect("last-good Claude observation should persist");
    }
    for (account_id, route_band, last_success) in [
        (&claude_account_id, RouteBand::ClaudeMessages, 800),
        (&openai_account_id, RouteBand::Responses, 900),
    ] {
        state
            .record_refresh_success_status(account_id, route_band.as_str(), last_success, 2_000)
            .await
            .expect("provider refresh success should persist");
    }
    for (account_id, route_band, error_class) in [
        (
            &claude_account_id,
            RouteBand::ClaudeMessages,
            QuotaRefreshErrorClass::ParseError,
        ),
        (
            &claude_account_id,
            RouteBand::Responses,
            QuotaRefreshErrorClass::NetworkError,
        ),
        (
            &openai_account_id,
            RouteBand::ClaudeMessages,
            QuotaRefreshErrorClass::RateLimited,
        ),
    ] {
        state
            .record_refresh_failure_preserving_selector_windows(
                account_id,
                route_band.as_str(),
                1_000,
                error_class,
            )
            .await
            .expect("refresh failure should persist");
    }

    for (now_unix_seconds, expected_freshness, expected_window_status) in [
        (
            1_100,
            QuotaEvidenceFreshness::Fresh,
            QuotaWindowStatus::Eligible,
        ),
        (
            1_500,
            QuotaEvidenceFreshness::Fresh,
            QuotaWindowStatus::Eligible,
        ),
        (
            1_501,
            QuotaEvidenceFreshness::Stale,
            QuotaWindowStatus::Stale,
        ),
    ] {
        let report = load_quota_status_report_with_availability_async(
            &router_root,
            false,
            now_unix_seconds,
            false,
            CredentialStoreAvailability::Ready,
        )
        .await
        .expect("quota report should load persisted provider state");
        let claude_row = report
            .rows()
            .iter()
            .find(|row| row.account_id == claude_account_id)
            .expect("Claude account should appear");
        assert!(
            claude_row.updated.contains("failed"),
            "{}",
            claude_row.updated
        );
        assert!(
            claude_row.updated.contains(": parse"),
            "{}",
            claude_row.updated
        );
        assert!(
            claude_row.updated.starts_with("ok "),
            "{}",
            claude_row.updated
        );
        assert_eq!(claude_row.freshness, expected_freshness);
        assert_eq!(claude_row.windows.len(), claude_observations.len());
        for (window_seconds, expected_headroom, expected_reset) in [
            (V1_SHORT_WINDOW_SECONDS, 60, 19_000),
            (V1_WEEKLY_WINDOW_SECONDS, 70, 605_000),
        ] {
            let window = claude_row
                .windows
                .iter()
                .find(|window| window.window_seconds == window_seconds)
                .expect("last-good window should appear");
            assert_eq!(window.status, expected_window_status);
            assert_eq!(window.remaining_headroom, expected_headroom);
            assert_eq!(window.reset_unix_seconds, Some(expected_reset));
            assert_eq!(window.observed_unix_seconds, 900);
        }
        let openai_row = report
            .rows()
            .iter()
            .find(|row| row.account_id == openai_account_id)
            .expect("OpenAI account should appear");
        assert!(
            openai_row.updated.starts_with("ok "),
            "{}",
            openai_row.updated
        );
        assert!(
            !openai_row.updated.contains("failed"),
            "{}",
            openai_row.updated
        );
    }
    let preserved_observations = state
        .window_observations_for_account(&claude_account_id)
        .await
        .expect("last-good observations should reload");
    assert_eq!(preserved_observations.len(), claude_observations.len());
    for observation in &claude_observations {
        assert!(preserved_observations.contains(observation));
    }
    state.close().await.expect("state store should close");
}

#[tokio::test]
async fn quota_status_report_reloads_saved_claude_credit_policy_from_sqlite() {
    let test_root = TempDir::new().expect("quota status root");
    let router_root = test_root.path().join("router");
    std::fs::create_dir_all(&router_root).expect("router root");
    let account_id = codex_router_core::ids::AccountId::new("saved_claude_credit_policy")
        .expect("Claude account id should parse");
    let state = AsyncSqliteStateStore::open(&router_root.join("state.sqlite"))
        .await
        .expect("state store should open");
    state
        .upsert_account(
            &codex_router_state::account::AccountRecord::new(
                codex_router_core::provider::Provider::Claude,
                account_id.clone(),
                "claude-credit-policy",
                codex_router_state::account::AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("Claude account should persist");
    state
        .save_account_credit_usage_policy(
            &account_id,
            codex_router_core::credit_usage::CreditUsagePolicy::Allow,
        )
        .await
        .expect("saved Claude preference should persist");
    state.close().await.expect("state store should close");

    let report = load_quota_status_report_with_availability_async(
        &router_root,
        false,
        1_000,
        false,
        CredentialStoreAvailability::Ready,
    )
    .await
    .expect("quota report should reload saved policy");
    let row = report
        .rows()
        .iter()
        .find(|row| row.account_id == account_id)
        .expect("Claude account should appear in report");

    assert_eq!(
        row.credit_usage.policy,
        codex_router_core::credit_usage::CreditUsagePolicy::Allow
    );
    assert_eq!(
        row.credit_usage.provider_observation,
        codex_router_core::credit_usage::CreditProviderObservation::missing()
    );
    assert_eq!(row.credit_usage.freshness, CreditUsageFreshness::Unknown);
}

#[derive(Default)]
struct CountingKeychainAccess {
    items: Mutex<HashMap<(String, String), Vec<u8>>>,
    read_count: AtomicUsize,
    add_count: AtomicUsize,
}

impl KeychainAccess for CountingKeychainAccess {
    fn read_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        self.read_count.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .items
            .lock()
            .expect("Keychain items lock")
            .get(&(service.to_owned(), account.to_owned()))
            .cloned())
    }

    fn add_secret(
        &self,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        let mut items = self.items.lock().expect("Keychain items lock");
        if items.contains_key(&(service.to_owned(), account.to_owned())) {
            return Err(KeychainAccessError::Unavailable);
        }
        items.insert((service.to_owned(), account.to_owned()), secret.to_vec());
        self.add_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn quota_status_reload_and_reset_session_reuse_the_process_credential_store() {
    let test_root = TempDir::new().expect("quota status root");
    let router_root = test_root.path().join("router");
    std::fs::create_dir_all(&router_root).expect("router root");
    let state = AsyncSqliteStateStore::open(&router_root.join("state.sqlite"))
        .await
        .expect("state store");
    state.close().await.expect("state close");

    let secret_root = router_root.join("secrets");
    std::fs::create_dir_all(&secret_root).expect("secret root");
    std::fs::write(secret_root.join("format-v2.marker"), b"2\n").expect("format marker");
    let keychain = CountingKeychainAccess::default();
    let credential_store =
        EncryptedCredentialStore::open_for_process_with_keychain(&secret_root, &keychain)
            .expect("process credential store");
    assert_eq!(
        credential_store.status(),
        EncryptedCredentialStoreStatus::Ready
    );
    let reads_after_open = keychain.read_count.load(Ordering::SeqCst);
    let adds_after_open = keychain.add_count.load(Ordering::SeqCst);
    assert!(reads_after_open > 0, "startup should read the Keychain key");

    let credential_resources = QuotaCredentialResources::from_opened_store(credential_store);
    let report = load_quota_status_report_with_availability_async(
        &router_root,
        false,
        1_000,
        false,
        credential_resources.availability(),
    )
    .await
    .expect("initial quota report");
    assert_eq!(
        report.credential_store_availability,
        CredentialStoreAvailability::Ready
    );

    let reload_view_model = quota_status_view_model_loader(
        router_root.clone(),
        false,
        false,
        100,
        credential_resources.availability(),
    );
    assert!(reload_view_model().await.is_some());

    let reset_session = FixedOriginInteractiveResetSessionFactory
        .create(
            &router_root,
            credential_resources
                .credential_store()
                .expect("opened process store"),
        )
        .expect("reset session composition");
    drop(reset_session);

    assert_eq!(keychain.read_count.load(Ordering::SeqCst), reads_after_open);
    assert_eq!(keychain.add_count.load(Ordering::SeqCst), adds_after_open);
}
