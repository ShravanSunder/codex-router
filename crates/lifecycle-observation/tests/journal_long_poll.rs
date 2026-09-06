use communication_protocol::LifecycleObservation;
use lifecycle_observation::{JournalPosition, LifecycleStore, ObservationJournal};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn reader_wakes_after_commit_without_holding_writer_lock() {
    let path = std::path::PathBuf::from(format!("/tmp/journal-wait-{}.sqlite", std::process::id()));
    let id = "00000000-0000-4000-8000-000000000001"
        .to_owned()
        .try_into()
        .unwrap_or_else(|e| panic!("id: {e}"));
    let journal = ObservationJournal::open(&path, id)
        .await
        .unwrap_or_else(|e| panic!("open: {e}"));
    let position = JournalPosition {
        journal_id: journal.journal_id().clone(),
        sequence: 0,
    };
    let store = Arc::new(LifecycleStore::new(journal));
    let endpoint =
        json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"});
    let target =
        serde_json::from_value(endpoint.clone()).unwrap_or_else(|e| panic!("endpoint: {e}"));
    let reader_store = Arc::clone(&store);
    let reader =
        tokio::spawn(async move { reader_store.read_wait(&target, position, 100, 1000).await });
    let observation:LifecycleObservation=serde_json::from_value(json!({"observedAt":"2026-09-05T12:00:00Z","source":"observerLifecycle","scope":{"endpoint":endpoint,"generation":null,"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"backend"},"change":{"kind":"coverageLost"}})).unwrap_or_else(|e|panic!("observation: {e}"));
    store
        .append(&observation, 100)
        .await
        .unwrap_or_else(|e| panic!("append: {e}"));
    let page = tokio::time::timeout(Duration::from_secs(2), reader)
        .await
        .unwrap_or_else(|e| panic!("timeout: {e}"))
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("read: {e}"));
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.next.sequence, 1);
    let target = observation.scope.endpoint.clone();
    let position = page.next;
    let empty = tokio::time::timeout(
        Duration::from_secs(1),
        store.read_wait(&target, position, 100, 10),
    )
    .await
    .unwrap_or_else(|e| panic!("bounded wait: {e}"))
    .unwrap_or_else(|e| panic!("timeout page: {e}"));
    assert!(empty.records.is_empty());
    assert!(empty.caught_up);
    assert_eq!(empty.next.sequence, 1);
    Arc::try_unwrap(store)
        .unwrap_or_else(|_| panic!("store still shared"))
        .close()
        .await;
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
