use super::*;

#[tokio::test]
async fn session_account_affinity_v12_migrates_to_provider_scoped_rows() {
    let temp_dir = TestTempDir::new("session_account_affinity_v12_to_v13");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("current fixture should open");
    store.close().await.expect("fixture should close");
    convert_current_fixture_to_v12(&database_path);

    let migrated = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("v12 database should migrate");

    assert_eq!(migrated.schema_version().await, Ok(13));
    migrated.close().await.expect("migrated store should close");
    let raw = Connection::open(&database_path).expect("migrated database should inspect");
    let mut statement = raw
        .prepare("PRAGMA table_info(session_account_affinities)")
        .expect("session affinity schema should prepare");
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("session affinity columns should query")
        .collect::<Result<Vec<_>, _>>()
        .expect("session affinity columns should load");
    assert_eq!(
        columns,
        vec![
            "provider",
            "session_id",
            "account_id",
            "last_seen_unix_seconds",
            "pin_version"
        ]
    );
}

#[tokio::test]
async fn session_account_affinity_upsert_replaces_account_and_last_seen() {
    let temp_dir = TestTempDir::new("session_account_affinity_upsert");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state store should open");
    let first = SessionAccountAffinity::new(
        codex_router_core::provider::Provider::Openai,
        "session-123",
        account_id("acct_first"),
        1_000,
    );
    let replacement = SessionAccountAffinity::new(
        codex_router_core::provider::Provider::Openai,
        "session-123",
        account_id("acct_replacement"),
        1_100,
    );

    AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(&store, &first)
        .await
        .expect("first mapping should persist");
    AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(&store, &replacement)
        .await
        .expect("replacement mapping should persist");

    assert_eq!(
        AsyncSessionAccountAffinityRepository::load_session_account_affinity(
            &store,
            Provider::Openai,
            "session-123",
        )
        .await,
        Ok(Some(replacement))
    );
}

#[tokio::test]
async fn session_account_affinity_cas_allows_one_writer_for_the_same_observation() {
    let temp_dir = TestTempDir::new("session_account_affinity_cas_race");
    let database_path = temp_dir.path().join("state.sqlite");
    let first_store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("first state store should open");
    let second_store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("second state store should open");
    let observation = PinObservation::new(None, 0);
    let first_writer = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        "racing-session",
        Some(account_id("acct_first_writer")),
        1,
        10_000,
    );
    let second_writer = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        "racing-session",
        Some(account_id("acct_second_writer")),
        1,
        10_000,
    );

    let (first_result, second_result) = tokio::join!(
        AsyncSessionAccountAffinityRepository::compare_and_set_session_account_affinity(
            &first_store,
            &observation,
            &first_writer,
            4_500,
        ),
        AsyncSessionAccountAffinityRepository::compare_and_set_session_account_affinity(
            &second_store,
            &observation,
            &second_writer,
            4_500,
        ),
    );
    let first_won = first_result.expect("first writer should reach the compare-and-set");
    let second_won = second_result.expect("second writer should reach the compare-and-set");

    assert_ne!(first_won, second_won);
    let stored_affinity = AsyncSessionAccountAffinityRepository::load_session_account_affinity(
        &first_store,
        Provider::Claude,
        "racing-session",
    )
    .await
    .expect("winning affinity should load")
    .expect("one writer should create the affinity");
    let winning_account = if first_won {
        first_writer.account_id()
    } else {
        second_writer.account_id()
    };
    assert_eq!(stored_affinity.account_id(), winning_account);
    assert_eq!(stored_affinity.pin_version(), 1);

    first_store.close().await.expect("first store should close");
    second_store
        .close()
        .await
        .expect("second store should close");
}

#[tokio::test]
async fn session_account_affinity_cas_rejects_late_writes_after_expiry_and_release() {
    let temp_dir = TestTempDir::new("session_account_affinity_cas_expiry_release");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state store should open");
    let expired_account = account_id("acct_expired");
    let initial_pin = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        "expiry-session",
        Some(expired_account.clone()),
        7,
        1_000,
    );
    AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(&store, &initial_pin)
        .await
        .expect("initial pin should persist");

    let stale_active_observation = PinObservation::new(Some(expired_account.clone()), 7);
    let late_renewal = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        "expiry-session",
        Some(expired_account),
        7,
        5_501,
    );
    assert!(
        !AsyncSessionAccountAffinityRepository::compare_and_set_session_account_affinity(
            &store,
            &stale_active_observation,
            &late_renewal,
            4_500,
        )
        .await
        .expect("late renewal should be rejected without a storage error")
    );

    let expired_observation = PinObservation::new(None, 7);
    let replacement_account = account_id("acct_replacement");
    let replacement_pin = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        "expiry-session",
        Some(replacement_account.clone()),
        8,
        5_501,
    );
    assert!(
        AsyncSessionAccountAffinityRepository::compare_and_set_session_account_affinity(
            &store,
            &expired_observation,
            &replacement_pin,
            4_500,
        )
        .await
        .expect("fresh observation should claim the expired pin")
    );

    let active_replacement_observation = PinObservation::new(Some(replacement_account.clone()), 8);
    let released_pin =
        SessionAccountAffinity::with_pin_state(Provider::Claude, "expiry-session", None, 9, 5_502);
    assert!(
        AsyncSessionAccountAffinityRepository::compare_and_set_session_account_affinity(
            &store,
            &active_replacement_observation,
            &released_pin,
            4_500,
        )
        .await
        .expect("release should compare-and-set the active pin")
    );

    let late_success_after_release = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        "expiry-session",
        Some(replacement_account),
        8,
        5_503,
    );
    assert!(
        !AsyncSessionAccountAffinityRepository::compare_and_set_session_account_affinity(
            &store,
            &active_replacement_observation,
            &late_success_after_release,
            4_500,
        )
        .await
        .expect("stale success should be rejected without a storage error")
    );

    let stored_pin = AsyncSessionAccountAffinityRepository::load_session_account_affinity(
        &store,
        Provider::Claude,
        "expiry-session",
    )
    .await
    .expect("released pin should load")
    .expect("released pin row should remain persisted");
    assert_eq!(stored_pin.account_id(), None);
    assert_eq!(stored_pin.pin_version(), 9);
    store.close().await.expect("state store should close");
}

#[tokio::test]
async fn session_account_affinities_are_provider_scoped_and_retain_released_rows() {
    let temp_dir = TestTempDir::new("provider_scoped_session_affinity");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state store should open");
    let openai_affinity = SessionAccountAffinity::new(
        codex_router_core::provider::Provider::Openai,
        "same-session",
        account_id("acct_openai"),
        1_000,
    );
    let claude_affinity = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        "same-session",
        Some(account_id("acct_claude")),
        7,
        2_000,
    );
    let released_affinity = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        "released-session",
        None,
        8,
        3_000,
    );
    for affinity in [&openai_affinity, &claude_affinity, &released_affinity] {
        AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(&store, affinity)
            .await
            .expect("provider-scoped affinity should persist");
    }

    let loaded_openai = AsyncSessionAccountAffinityRepository::load_session_account_affinity(
        &store,
        Provider::Openai,
        "same-session",
    )
    .await
    .expect("OpenAI affinity should load")
    .expect("OpenAI pin should exist");
    let loaded_claude = AsyncSessionAccountAffinityRepository::load_session_account_affinity(
        &store,
        Provider::Claude,
        "same-session",
    )
    .await
    .expect("Claude affinity should load")
    .expect("Claude pin should exist");
    let loaded_release = AsyncSessionAccountAffinityRepository::load_session_account_affinity(
        &store,
        Provider::Claude,
        "released-session",
    )
    .await
    .expect("released affinity should load")
    .expect("released pin row should persist");

    assert_eq!(
        loaded_openai.account_id().map(AccountId::as_str),
        Some("acct_openai")
    );
    assert_eq!(
        loaded_claude.account_id().map(AccountId::as_str),
        Some("acct_claude")
    );
    assert_eq!(loaded_claude.pin_version(), 7);
    assert_eq!(loaded_release.account_id(), None);
    assert_eq!(loaded_release.pin_version(), 8);
    store.close().await.expect("state store should close");
}

#[tokio::test]
async fn session_account_affinity_purge_deletes_only_rows_before_cutoff() {
    let temp_dir = TestTempDir::new("session_account_affinity_purge");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state store should open");
    for affinity in [
        SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-old",
            account_id("acct_old"),
            999,
        ),
        SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-cutoff",
            account_id("acct_cutoff"),
            1_000,
        ),
        SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-fresh",
            account_id("acct_fresh"),
            1_001,
        ),
    ] {
        AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(&store, &affinity)
            .await
            .expect("session affinity should persist");
    }

    store
        .purge_session_account_affinities_before(1_000)
        .await
        .expect("old session affinities should purge");

    assert_eq!(
        AsyncSessionAccountAffinityRepository::load_session_account_affinity(
            &store,
            Provider::Openai,
            "session-old",
        )
        .await,
        Ok(None)
    );
    for retained_session_id in ["session-cutoff", "session-fresh"] {
        assert!(
            AsyncSessionAccountAffinityRepository::load_session_account_affinity(
                &store,
                Provider::Openai,
                retained_session_id,
            )
            .await
            .expect("retained affinity should load")
            .is_some()
        );
    }
    assert_eq!(store.schema_version().await, Ok(13));
    store.close().await.expect("state store should close");

    let raw = Connection::open(&database_path).expect("database should inspect");
    let index_count: i64 = raw
        .query_row(
            "SELECT COUNT(*)
                   FROM pragma_index_list('session_account_affinities')
                  WHERE name = 'session_account_affinities_last_seen_lookup'",
            [],
            |row| row.get(0),
        )
        .expect("session affinity indexes should query");
    assert_eq!(index_count, 1);
}
