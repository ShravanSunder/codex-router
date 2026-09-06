use communication_protocol::LifecycleObservation;
use lifecycle_observation::ObservationJournal;

#[tokio::test]
async fn invalid_coverage_restore_does_not_commit_or_consume_sequence() {
    // Arrange: a real SQLite journal and a restore with no accepting generation.
    let path = std::path::PathBuf::from(format!(
        "/tmp/invalid-coverage-{}.sqlite",
        std::process::id()
    ));
    let identity = "00000000-0000-4000-8000-000000000001"
        .to_owned()
        .try_into()
        .unwrap_or_else(|error| panic!("identity: {error}"));
    let journal = ObservationJournal::open(&path, identity)
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));
    let position = lifecycle_observation::JournalPosition {
        journal_id: journal.journal_id().clone(),
        sequence: 0,
    };
    let store = lifecycle_observation::LifecycleStore::new(journal);
    let mut observation: LifecycleObservation = serde_json::from_value(serde_json::json!({
        "observedAt": "2026-09-05T12:00:00Z",
        "source": "observerLifecycle",
        "scope": {
            "endpoint": {"serviceId": "00000000-0000-4000-8000-000000000001", "endpointId": "codex-local"},
            "generation": null,
            "observerId": "00000000-0000-4000-8000-000000000002"
        },
        "subject": {"kind": "backend"},
        "change": {"kind": "coverageRestored"}
    })).unwrap_or_else(|error| panic!("observation: {error}"));

    // Act: submit invalid coverage through the actual storage owner.
    let result = store.append(&observation, 100).await;
    let page = store
        .read_wait(&observation.scope.endpoint, position, 100, 0)
        .await
        .unwrap_or_else(|error| panic!("read: {error}"));
    observation.change = communication_protocol::LifecycleChange::CoverageLost;
    let next = store
        .append(&observation, 80)
        .await
        .unwrap_or_else(|error| panic!("valid append: {error}"));
    store.close().await;
    std::fs::remove_file(path).unwrap_or_else(|error| panic!("cleanup: {error}"));

    // Assert: rejection has no persisted record, sequence or retention-clock effect.
    assert!(matches!(
        result,
        Err(lifecycle_observation::JournalError::InvalidRecord)
    ));
    assert!(page.records.is_empty());
    assert_eq!(page.next.sequence, 0);
    assert_eq!(next.sequence, 1);
    assert_eq!(next.retention_at, 80);
}

#[tokio::test]
async fn journal_reopens_without_reusing_sequence_and_keeps_retention_monotonic() {
    let path = std::path::PathBuf::from(format!(
        "/tmp/lifecycle-journal-{}.sqlite",
        std::process::id()
    ));
    let id = || {
        "00000000-0000-4000-8000-000000000001"
            .to_owned()
            .try_into()
            .unwrap_or_else(|e| panic!("id: {e}"))
    };
    let observation:LifecycleObservation=serde_json::from_str(r#"{"observedAt":"2026-09-05T12:00:00Z","source":"observerLifecycle","scope":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":null,"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"backend"},"change":{"kind":"coverageLost"}}"#).unwrap_or_else(|e|panic!("observation: {e}"));
    let mut journal = ObservationJournal::open(&path, id())
        .await
        .unwrap_or_else(|e| panic!("open: {e}"));
    let first = journal
        .append(&observation, 100)
        .await
        .unwrap_or_else(|e| panic!("append: {e}"));
    assert_eq!(first.sequence, 1);
    journal.close().await;
    let mut journal = ObservationJournal::open(&path, id())
        .await
        .unwrap_or_else(|e| panic!("reopen: {e}"));
    let second = journal
        .append(&observation, 80)
        .await
        .unwrap_or_else(|e| panic!("append: {e}"));
    assert_eq!(second.sequence, 2);
    assert_eq!(second.retention_at, 100);
    let records = journal
        .read_after(0, 100)
        .await
        .unwrap_or_else(|e| panic!("read: {e}"));
    assert_eq!(records.len(), 2);
    assert_eq!(
        records.first().map(|row| &row.observation),
        Some(&observation)
    );
    journal.close().await;
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
