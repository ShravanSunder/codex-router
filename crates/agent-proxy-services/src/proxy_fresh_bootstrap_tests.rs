use crate::proxy_role_test_fixtures::*;
use codex_router_keeper_protocol::PrepareMode;
use codex_router_state::{
    account::{AccountRecord, AccountStatus},
    sqlite::{AsyncSqliteStateStore, SqliteStateStore},
};
use std::time::{Duration, Instant};
#[tokio::test]
async fn fresh_recognized_legacy_prepares_without_mutation_then_migrates_and_serves() {
    let root = tempfile::tempdir().expect("isolated legacy root");
    drop(
        prepare(root.path(), PrepareMode::Fresh)
            .await
            .expect("permitted secret bootstrap before state scenario"),
    );
    let account_id =
        codex_router_core::ids::AccountId::new("fresh-legacy-account").expect("literal account id");
    let legacy = SqliteStateStore::open(&root.path().join("state.sqlite"))
        .expect("existing supported legacy owner");
    legacy
        .upsert_account(
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                "preserved fresh legacy",
                AccountStatus::Disabled,
            )
            .with_active_credential_generation(41),
        )
        .expect("representative legacy row");
    drop(legacy);
    let before = snapshot(root.path());
    let prepared = prepare(root.path(), PrepareMode::Fresh)
        .await
        .expect("Fresh must preserve recognized legacy startup support");
    assert!(
        matches!(
            prepared.state_schema(),
            crate::ProxyPreparedStateSchema::FreshBootstrap {
                kind:
                    codex_router_state::schema_preparation::AccountBootstrapKind::RecognizedLegacy
            }
        ),
        "bootstrap observation must not pretend native Current/Pending"
    );
    assert!(
        snapshot(root.path()) == before,
        "Fresh inspection cannot change state/schema/history/data or stable secret identities"
    );
    let began = Instant::now();
    let mut active = prepared
        .activate_for_test()
        .await
        .expect("existing migration authority at Activate");
    let elapsed = began.elapsed();
    eprintln!("Fresh legacy activation elapsed_us={}", elapsed.as_micros());
    assert!(
        elapsed <= Duration::from_millis(500),
        "ACTIVATE_DEADLINE exceeded: {elapsed:?}"
    );
    let state = AsyncSqliteStateStore::open_read_only(&root.path().join("state.sqlite"))
        .await
        .expect("native schema after Activate");
    let row = state
        .load_account(&account_id)
        .await
        .expect("actual typed row")
        .expect("legacy account preserved");
    assert_eq!(row.label(), "preserved fresh legacy");
    assert_eq!(row.active_credential_generation(), Some(41));
    state.close().await.expect("fixture typed read closes");
    assert!(
        http(active.local_addr(), HEALTH_REQUEST)
            .await
            .starts_with(b"HTTP/1.1 200")
    );
    active.shutdown().await.expect("owned role cleanup");
}
