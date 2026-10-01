use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_router_state::sqlite::AsyncSessionAccountAffinityRepository;
use codex_router_state::sqlite::AsyncSqliteStateStore;

use super::*;

static NEXT_TEST_DATABASE: AtomicUsize = AtomicUsize::new(0);

fn account_id(value: &str) -> AccountId {
    AccountId::new(value).unwrap_or_else(|error| panic!("test account id: {error}"))
}

fn test_database_path(name: &str) -> PathBuf {
    let process_id = std::process::id();
    let counter = NEXT_TEST_DATABASE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "codex-router-proxy-session-affinity-{name}-{process_id}-{counter}.sqlite"
    ))
}

fn completed_response(
    completion: ClaudeResponseCompletion,
) -> tokio::sync::oneshot::Receiver<ClaudeResponseCompletion> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    sender
        .send(completion)
        .unwrap_or_else(|_| panic!("completion receiver should remain open"));
    receiver
}

#[test]
fn default_ttl_expires_after_75_idle_minutes_without_renewal() {
    for (age, expected_fresh) in [(4_499, true), (4_500, false), (4_501, false)] {
        let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
        let selected = publish_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-boundary",
            &account_id("acct-a"),
            RouteBand::Responses,
            None,
            1_000,
        )
        .unwrap_or_else(|_| panic!("publication should succeed"));
        drop(selected);

        let lookup = lookup_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-boundary",
            RouteBand::Responses,
            None,
            1_000 + age,
        )
        .unwrap_or_else(|_| panic!("lookup should succeed"));
        assert_eq!(lookup.is_some(), expected_fresh, "age={age}");
    }
}

#[test]
fn configured_ttl_is_used_for_expiry() {
    let cache = SessionAccountAffinityCache::shared(Duration::from_secs(30 * 60));
    let _selected = publish_session_account_affinity(
        &cache,
        Provider::Openai,
        "session-configured-ttl",
        &account_id("acct-a"),
        RouteBand::Responses,
        None,
        1_000,
    )
    .unwrap_or_else(|_| panic!("publication should succeed"));

    for (age, expected_fresh) in [(1_799, true), (1_800, false)] {
        let lookup = lookup_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-configured-ttl",
            RouteBand::Responses,
            None,
            1_000 + age,
        )
        .unwrap_or_else(|_| panic!("lookup should succeed"));
        assert_eq!(lookup.is_some(), expected_fresh, "age={age}");
    }
}

#[test]
fn same_session_id_has_independent_provider_owners() {
    let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
    let openai_selection = publish_session_account_affinity(
        &cache,
        Provider::Openai,
        "shared-session-id",
        &account_id("acct-openai"),
        RouteBand::Responses,
        None,
        1_000,
    )
    .unwrap_or_else(|_| panic!("OpenAI publication should succeed"));
    let _claude_selection = publish_session_account_affinity(
        &cache,
        Provider::Claude,
        "shared-session-id",
        &account_id("acct-claude"),
        RouteBand::Responses,
        None,
        1_100,
    )
    .unwrap_or_else(|_| panic!("Claude publication should succeed"));

    let openai_lookup = lookup_session_account_affinity(
        &cache,
        Provider::Openai,
        "shared-session-id",
        RouteBand::Responses,
        None,
        1_101,
    )
    .unwrap_or_else(|_| panic!("OpenAI lookup should succeed"));
    assert_eq!(
        openai_lookup.map(|selection| selection.account_id().clone()),
        Some(openai_selection.account_id().clone())
    );
}

#[test]
fn stale_handle_cannot_renew_after_a_to_b_to_a() {
    let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
    let original_a = publish_session_account_affinity(
        &cache,
        Provider::Openai,
        "session-cycle",
        &account_id("acct-a"),
        RouteBand::Responses,
        None,
        1_000,
    )
    .unwrap_or_else(|_| panic!("A publication should succeed"));
    let _b = publish_session_account_affinity(
        &cache,
        Provider::Openai,
        "session-cycle",
        &account_id("acct-b"),
        RouteBand::Responses,
        None,
        1_100,
    )
    .unwrap_or_else(|_| panic!("B publication should succeed"));
    let current_a = publish_session_account_affinity(
        &cache,
        Provider::Openai,
        "session-cycle",
        &account_id("acct-a"),
        RouteBand::Responses,
        None,
        1_200,
    )
    .unwrap_or_else(|_| panic!("second A publication should succeed"));

    assert!(
        !original_a
            .activity_handle()
            .touch_if_current(2_000)
            .unwrap()
    );
    assert!(current_a.activity_handle().touch_if_current(2_000).unwrap());
}

#[test]
fn persisted_reconciliation_preserves_last_seen_and_live_touch_wins() {
    let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
    let persisted = SessionAccountAffinity::new(
        codex_router_core::provider::Provider::Openai,
        "session-db",
        account_id("acct-a"),
        1_000,
    );
    let seeded = reconcile_persisted_session_account_affinity(
        &cache,
        Provider::Openai,
        "session-db",
        Some(&persisted),
        RouteBand::Responses,
        None,
        5_499,
    )
    .unwrap_or_else(|_| panic!("reconciliation should succeed"))
    .unwrap_or_else(|| panic!("4,499-second row should seed"));
    assert!(
        lookup_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-db",
            RouteBand::Responses,
            None,
            5_500,
        )
        .unwrap_or_else(|_| panic!("lookup-only boundary check should succeed"))
        .is_none(),
        "seeding at age 4,499 must preserve persisted last-seen and expire at age 4,500"
    );
    assert!(seeded.activity_handle().touch_if_current(5_600).unwrap());

    let older = SessionAccountAffinity::new(
        codex_router_core::provider::Provider::Openai,
        "session-db",
        account_id("acct-b"),
        8_250,
    );
    let reconciled = reconcile_persisted_session_account_affinity(
        &cache,
        Provider::Openai,
        "session-db",
        Some(&older),
        RouteBand::Responses,
        None,
        5_601,
    )
    .unwrap_or_else(|_| panic!("second reconciliation should succeed"))
    .unwrap_or_else(|| panic!("live owner should remain"));
    assert_eq!(reconciled.account_id().as_str(), "acct-a");
}

#[test]
fn persisted_reconciliation_uses_default_75_minute_ttl() {
    for (age, expected_fresh) in [(4_499, true), (4_500, false), (4_501, false)] {
        let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
        let persisted = SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-db-boundary",
            account_id("acct-a"),
            1_000,
        );
        let reconciled = reconcile_persisted_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-db-boundary",
            Some(&persisted),
            RouteBand::Responses,
            None,
            1_000 + age,
        )
        .unwrap_or_else(|_| panic!("reconciliation should succeed"));
        assert_eq!(reconciled.is_some(), expected_fresh, "age={age}");
    }
}

#[test]
fn expired_entry_with_current_handle_can_renew_after_real_activity() {
    let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
    let published = publish_session_account_affinity(
        &cache,
        Provider::Openai,
        "session-idle-activity",
        &account_id("acct-a"),
        RouteBand::Responses,
        None,
        1_000,
    )
    .unwrap_or_else(|_| panic!("publication should succeed"));

    assert!(
        lookup_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-idle-activity",
            RouteBand::Responses,
            None,
            5_500,
        )
        .unwrap_or_else(|_| panic!("lookup should succeed"))
        .is_none()
    );
    assert!(
        published
            .activity_handle()
            .touch_if_current(5_600)
            .unwrap_or_else(|_| panic!("touch should succeed"))
    );
    assert!(
        lookup_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-idle-activity",
            RouteBand::Responses,
            None,
            5_600,
        )
        .unwrap_or_else(|_| panic!("lookup should succeed"))
        .is_some()
    );
}

#[test]
fn claude_selection_does_not_publish_or_renew_before_success() {
    let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
    let selected = publish_session_account_affinity(
        &cache,
        Provider::Claude,
        "session-before-success",
        &account_id("acct-claude"),
        RouteBand::Responses,
        None,
        1_000,
    )
    .unwrap_or_else(|_| panic!("selection should succeed"));

    assert!(
        lookup_session_account_affinity(
            &cache,
            Provider::Claude,
            "session-before-success",
            RouteBand::Responses,
            None,
            1_001,
        )
        .unwrap_or_else(|_| panic!("lookup should succeed"))
        .is_none(),
        "Claude selection alone must not publish an active pin"
    );
    assert!(
        !selected
            .activity_handle()
            .touch_if_current(1_002)
            .unwrap_or_else(|_| panic!("activity check should succeed")),
        "Claude activity must not renew a pin before typed success evidence"
    );
}

fn affinity(
    session_id: &str,
    account_id: Option<AccountId>,
    pin_version: u64,
    last_seen_unix_seconds: u64,
) -> SessionAccountAffinity {
    SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        session_id,
        account_id,
        pin_version,
        last_seen_unix_seconds,
    )
}

async fn save_pin(store: &AsyncSqliteStateStore, pin: &SessionAccountAffinity) {
    AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(store, pin)
        .await
        .unwrap_or_else(|error| panic!("pin should persist: {error}"));
}

async fn assert_pin(
    store: &AsyncSqliteStateStore,
    session_id: &str,
    expected: &SessionAccountAffinity,
) {
    assert_eq!(
        AsyncSessionAccountAffinityRepository::load_session_account_affinity(
            store,
            Provider::Claude,
            session_id,
        )
        .await
        .unwrap_or_else(|error| panic!("pin should load: {error}")),
        Some(expected.clone())
    );
}

async fn publish(
    cache: &SharedSessionAccountAffinityCache,
    store: &AsyncSqliteStateStore,
    session_id: &str,
    observation: &PinObservation,
    attempt_account: &AccountId,
    completion: ClaudeResponseCompletion,
    now_unix_seconds: u64,
) -> bool {
    publish_claude_session_account_affinity_on_success(
        cache,
        session_id,
        observation,
        attempt_account,
        completed_response(completion),
        store,
        || now_unix_seconds,
    )
    .await
    .unwrap_or_else(|error| panic!("pin publication should complete: {error}"))
}

#[tokio::test]
async fn claude_success_only_creates_or_renews_the_authorized_pin() {
    let store = AsyncSqliteStateStore::open(&test_database_path("success_and_no_success"))
        .await
        .unwrap_or_else(|error| panic!("test state should open: {error}"));
    let cache = SessionAccountAffinityCache::shared(Duration::from_secs(4_500));
    let account = account_id("acct-success");

    for completion in [
        ClaudeResponseCompletion::Incomplete,
        ClaudeResponseCompletion::ProviderError,
    ] {
        assert!(
            !publish(
                &cache,
                &store,
                "session-unsuccessful",
                &PinObservation::new(None, 0),
                &account,
                completion,
                1_000,
            )
            .await
        );
    }
    assert!(
        AsyncSessionAccountAffinityRepository::load_session_account_affinity(
            &store,
            Provider::Claude,
            "session-unsuccessful",
        )
        .await
        .unwrap_or_else(|error| panic!("unsuccessful pin read should succeed: {error}"))
        .is_none()
    );

    assert!(
        publish(
            &cache,
            &store,
            "session-success",
            &PinObservation::new(None, 0),
            &account,
            ClaudeResponseCompletion::Success,
            1_000,
        )
        .await
    );
    assert_pin(
        &store,
        "session-success",
        &affinity("session-success", Some(account.clone()), 1, 1_000),
    )
    .await;

    assert!(
        publish(
            &cache,
            &store,
            "session-success",
            &PinObservation::new(Some(account.clone()), 1),
            &account,
            ClaudeResponseCompletion::Success,
            1_100,
        )
        .await
    );
    assert_pin(
        &store,
        "session-success",
        &affinity("session-success", Some(account.clone()), 1, 1_100),
    )
    .await;
    assert_eq!(
        lookup_session_account_affinity(
            &cache,
            Provider::Claude,
            "session-success",
            RouteBand::Responses,
            None,
            1_101,
        )
        .unwrap_or_else(|_| panic!("successful pin should be cached"))
        .map(|selection| selection.account_id().clone()),
        Some(account)
    );
    store
        .close()
        .await
        .unwrap_or_else(|error| panic!("test state should close: {error}"));
}

#[tokio::test]
async fn stale_mismatched_expired_and_released_authority_is_unchanged() {
    let store = AsyncSqliteStateStore::open(&test_database_path("authority_noop"))
        .await
        .unwrap_or_else(|error| panic!("test state should open: {error}"));
    let cache = SessionAccountAffinityCache::shared(Duration::from_secs(4_500));
    let account_a = account_id("acct-a");
    let account_b = account_id("acct-b");
    let other_account = account_id("acct-other");
    let stale_pin = affinity("session-stale", Some(account_a.clone()), 5, 1_000);
    let expired_pin = affinity("session-expired", Some(account_a.clone()), 2, 1_000);
    let active_pin = affinity("session-released", Some(account_b.clone()), 7, 1_000);
    for pin in [&stale_pin, &expired_pin, &active_pin] {
        save_pin(&store, pin).await;
    }

    for (session_id, observation, attempt_account, now) in [
        (
            "session-stale",
            PinObservation::new(Some(account_a.clone()), 4),
            account_a.clone(),
            1_100,
        ),
        (
            "session-stale",
            PinObservation::new(Some(account_a.clone()), 5),
            other_account,
            1_200,
        ),
        (
            "session-expired",
            PinObservation::new(Some(account_a.clone()), 2),
            account_a.clone(),
            5_500,
        ),
    ] {
        assert!(
            !publish(
                &cache,
                &store,
                session_id,
                &observation,
                &attempt_account,
                ClaudeResponseCompletion::Success,
                now,
            )
            .await,
            "session={session_id}"
        );
    }

    let released_pin = affinity("session-released", None, 8, 1_100);
    assert!(
        AsyncSessionAccountAffinityRepository::compare_and_set_session_account_affinity(
            &store,
            &PinObservation::new(Some(account_b.clone()), 7),
            &released_pin,
            4_500,
        )
        .await
        .unwrap_or_else(|error| panic!("release compare-and-set should execute: {error}"))
    );
    assert!(
        !publish(
            &cache,
            &store,
            "session-released",
            &PinObservation::new(Some(account_b.clone()), 7),
            &account_b,
            ClaudeResponseCompletion::Success,
            1_200,
        )
        .await
    );

    for (session_id, expected) in [
        ("session-stale", stale_pin),
        ("session-expired", expired_pin),
        ("session-released", released_pin),
    ] {
        assert_pin(&store, session_id, &expected).await;
        assert!(
            lookup_session_account_affinity(
                &cache,
                Provider::Claude,
                session_id,
                RouteBand::Responses,
                None,
                5_500,
            )
            .unwrap_or_else(|_| panic!("unchanged pin lookup should succeed"))
            .is_none(),
            "no-op publication must not mutate the cache"
        );
    }
    store
        .close()
        .await
        .unwrap_or_else(|error| panic!("test state should close: {error}"));
}
