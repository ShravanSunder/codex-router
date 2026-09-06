use communication_protocol::LifecycleObservation;
use lifecycle_observation::{JournalError, ObservationJournal};
use serde_json::json;
#[tokio::test]
async fn expired_prefix_keeps_checkpoint_and_rejects_old_cursor() {
    let path = std::path::PathBuf::from(format!(
        "/tmp/journal-retention-{}.sqlite",
        std::process::id()
    ));
    let id = || {
        "00000000-0000-4000-8000-000000000001"
            .to_owned()
            .try_into()
            .unwrap_or_else(|e| panic!("id: {e}"))
    };
    let endpoint =
        json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"});
    let address = json!({"endpoint":endpoint,"nativeThreadId":"thread"});
    let observation = |change| {
        serde_json::from_value::<LifecycleObservation>(json!({"observedAt":"2026-09-05T12:00:00Z","source":"nativeNotification","scope":{"endpoint":endpoint,"generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000003","generation":1},"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"thread","address":address},"change":change})).unwrap_or_else(|e|panic!("observation: {e}"))
    };
    let mut journal = ObservationJournal::open(&path, id())
        .await
        .unwrap_or_else(|e| panic!("open: {e}"));
    journal
        .append(&observation(json!({"kind":"threadDiscovered"})), 1)
        .await
        .unwrap_or_else(|e| panic!("append: {e}"));
    journal
        .append(&observation(json!({"kind":"threadArchived"})), 40 * 86400)
        .await
        .unwrap_or_else(|e| panic!("append: {e}"));
    assert_eq!(
        journal
            .expire_history(40 * 86400)
            .await
            .unwrap_or_else(|e| panic!("expire: {e}")),
        1
    );
    assert!(matches!(
        journal.read_after(0, 100).await,
        Err(JournalError::HistoryExpired)
    ));
    assert_eq!(
        journal
            .read_after(1, 100)
            .await
            .unwrap_or_else(|e| panic!("suffix: {e}"))
            .len(),
        1
    );
    journal.close().await;
    let mut journal = ObservationJournal::open(&path, id())
        .await
        .unwrap_or_else(|e| panic!("reopen: {e}"));
    assert_eq!(
        journal
            .expire_history(20 * 86400)
            .await
            .unwrap_or_else(|e| panic!("backward clock: {e}")),
        1
    );
    let target = serde_json::from_value(address).unwrap_or_else(|e| panic!("address: {e}"));
    assert!(
        journal
            .address_entry(&target)
            .await
            .unwrap_or_else(|e| panic!("address: {e}"))
            .is_some()
    );
    let before = serde_json::to_value(
        journal
            .address_entry(&target)
            .await
            .unwrap_or_else(|e| panic!("before: {e}")),
    )
    .unwrap_or_else(|e| panic!("serialize: {e}"));
    journal.close().await;
    use sqlx::Connection;
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&path);
    let mut fixture = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap_or_else(|e| panic!("fixture: {e}"));
    sqlx::query("DELETE FROM thread_addresses")
        .execute(&mut fixture)
        .await
        .unwrap_or_else(|e| panic!("remove derived rows: {e}"));
    fixture
        .close()
        .await
        .unwrap_or_else(|e| panic!("close: {e}"));
    let mut journal = ObservationJournal::open(&path, id())
        .await
        .unwrap_or_else(|e| panic!("open: {e}"));
    journal
        .rebuild_addresses()
        .await
        .unwrap_or_else(|e| panic!("rebuild: {e}"));
    let after = serde_json::to_value(
        journal
            .address_entry(&target)
            .await
            .unwrap_or_else(|e| panic!("after: {e}")),
    )
    .unwrap_or_else(|e| panic!("serialize: {e}"));
    assert_eq!(after, before);
    journal.close().await;
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
