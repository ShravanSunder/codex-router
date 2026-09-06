use communication_protocol::LifecycleObservation;
use lifecycle_observation::{JournalError, LifecycleStore, ObservationJournal};
use sqlx::Connection;

#[tokio::test]
async fn failed_append_disables_journal_publication_until_successful_recovery() {
    // Arrange: a real journal whose committed accounting reaches its logical capacity.
    let path = std::env::temp_dir().join(format!("journal-capacity-{}.sqlite", std::process::id()));
    let identity = "00000000-0000-4000-8000-000000000001"
        .to_owned()
        .try_into()
        .unwrap_or_else(|error| panic!("identity: {error}"));
    let journal = ObservationJournal::open(&path, identity)
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));
    let store = LifecycleStore::new(journal);
    let observation: LifecycleObservation = serde_json::from_value(serde_json::json!({"observedAt":"2026-09-05T12:00:00Z","source":"observerLifecycle","scope":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":null,"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"backend"},"change":{"kind":"coverageLost"}})).unwrap_or_else(|error| panic!("observation: {error}"));
    let mut accounting = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await
    .unwrap_or_else(|error| panic!("accounting connection: {error}"));
    sqlx::query("UPDATE journal_metadata SET payload_bytes=268435456 WHERE singleton=1")
        .execute(&mut accounting)
        .await
        .unwrap_or_else(|error| panic!("capacity: {error}"));
    // Act: capacity refusal leaves no record, but is still an observation coverage gap.
    let failed = store.append(&observation, 100).await;
    let unavailable = store.bounds().await;
    sqlx::query("UPDATE journal_metadata SET payload_bytes=0 WHERE singleton=1")
        .execute(&mut accounting)
        .await
        .unwrap_or_else(|error| panic!("restore capacity: {error}"));
    let before_recovery = store.bounds().await;
    store
        .prepare(100)
        .await
        .unwrap_or_else(|error| panic!("recovery: {error}"));
    let restored = store.bounds().await;
    accounting
        .close()
        .await
        .unwrap_or_else(|error| panic!("close accounting: {error}"));
    store.close().await;
    std::fs::remove_file(path).unwrap_or_else(|error| panic!("cleanup: {error}"));
    // Assert: readable metadata alone cannot declare ingestion healthy again.
    assert!(matches!(failed, Err(JournalError::Capacity)));
    assert!(unavailable.is_err());
    assert!(before_recovery.is_err());
    assert_eq!(
        restored
            .unwrap_or_else(|error| panic!("restored bounds: {error}"))
            .last_sequence,
        0
    );
}
