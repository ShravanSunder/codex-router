//! Capacity accounting pressure must attempt only policy-eligible history expiry.
#[cfg(test)]
mod tests {
    use communication_protocol::{LifecycleObservation, UuidIdentity};
    use lifecycle_observation::{JournalError, LifecycleStore, ObservationJournal};
    use serde_json::json;
    use sqlx::Connection;
    use std::path::Path;

    const NOW: i64 = 40 * 86400;

    #[tokio::test]
    async fn capacity_pressure_reclaims_expired_prefix_before_rejecting_append() {
        // Arrange: one expired and one recent record. Seed the logical byte counter
        // at its limit to exercise admission without a 256 MiB stress fixture.
        let path =
            std::env::temp_dir().join(format!("capacity-expired-{}.sqlite", std::process::id()));
        let mut journal = ObservationJournal::open(&path, identity()).await.unwrap();
        journal.append(&observation("old"), 1).await.unwrap();
        journal.append(&observation("recent"), NOW).await.unwrap();
        journal.close().await;
        seed_capacity_counter(&path).await;
        let journal = ObservationJournal::open(&path, identity()).await.unwrap();
        let store = LifecycleStore::new(journal);
        // Act: new record fits only after the expired prefix is reclaimed.
        let result = store.append(&observation("new"), NOW).await;
        // Assert: metadata acceptance remains usable, with exactly the expired prefix removed.
        assert!(
            result.is_ok(),
            "expired history must be reclaimed first: {:?}",
            result.as_ref().err()
        );
        let bounds = store.bounds().await.unwrap();
        assert_eq!(bounds.earliest_sequence, 2);
        assert_eq!(bounds.last_sequence, 3);
        store.close().await;
        let mut journal = ObservationJournal::open(&path, identity()).await.unwrap();
        assert!(matches!(
            journal.read_after(0, 100).await,
            Err(JournalError::HistoryExpired)
        ));
        let retained = journal.read_after(1, 100).await.unwrap();
        assert_eq!(retained.len(), 2);
        let address = serde_json::from_value(json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"nativeThreadId":"old"})).unwrap();
        assert!(
            journal.address_entry(&address).await.unwrap().is_some(),
            "checkpoint must retain the expired record's address"
        );
        journal.close().await;
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn capacity_pressure_never_discards_unexpired_history_to_fit() {
        // Arrange: capacity pressure with no policy-eligible records.
        let path =
            std::env::temp_dir().join(format!("capacity-recent-{}.sqlite", std::process::id()));
        let mut journal = ObservationJournal::open(&path, identity()).await.unwrap();
        journal.append(&observation("recent"), NOW).await.unwrap();
        journal.close().await;
        seed_capacity_counter(&path).await;
        let journal = ObservationJournal::open(&path, identity()).await.unwrap();
        let store = LifecycleStore::new(journal);
        // Act / Assert: fail closed and preserve unexpired input, without committing a new row.
        assert!(matches!(
            store.append(&observation("new"), NOW).await,
            Err(JournalError::Capacity)
        ));
        assert!(store.bounds().await.is_err());
        store.close().await;
        let mut journal = ObservationJournal::open(&path, identity()).await.unwrap();
        assert_eq!(journal.read_after(0, 100).await.unwrap().len(), 1);
        assert_eq!(journal.bounds().await.unwrap().last_sequence, 1);
        journal.close().await;
        std::fs::remove_file(path).unwrap();
    }

    fn identity() -> UuidIdentity {
        "00000000-0000-4000-8000-000000000001"
            .to_owned()
            .try_into()
            .unwrap()
    }
    fn observation(thread: &str) -> LifecycleObservation {
        serde_json::from_value(json!({
            "observedAt":"2026-09-06T12:00:00Z","source":"inventoryRead",
            "scope":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":null,"observerId":"00000000-0000-4000-8000-000000000002"},
            "subject":{"kind":"thread","address":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"nativeThreadId":thread}},
            "change":{"kind":"threadDiscovered"}
        })).unwrap()
    }
    async fn seed_capacity_counter(path: &Path) {
        let mut connection = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(path),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE journal_metadata SET payload_bytes=256*1024*1024 WHERE singleton=1")
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
    }
}
