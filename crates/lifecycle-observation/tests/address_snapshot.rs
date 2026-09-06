use communication_protocol::LifecycleObservation;
use lifecycle_observation::ObservationJournal;
use serde_json::json;
#[tokio::test]
async fn captured_addresses_do_not_change_when_journal_advances() {
    let path = std::path::PathBuf::from(format!(
        "/tmp/address-snapshot-{}.sqlite",
        std::process::id()
    ));
    let id = "00000000-0000-4000-8000-000000000001"
        .to_owned()
        .try_into()
        .unwrap_or_else(|e| panic!("id: {e}"));
    let endpoint =
        json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"});
    let observation = |thread| {
        serde_json::from_value::<LifecycleObservation>(json!({"observedAt":"2026-09-05T12:00:00Z","source":"inventoryRead","scope":{"endpoint":endpoint,"generation":null,"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"thread","address":{"endpoint":endpoint,"nativeThreadId":thread}},"change":{"kind":"threadDiscovered"}})).unwrap_or_else(|e|panic!("observation: {e}"))
    };
    let mut journal = ObservationJournal::open(&path, id)
        .await
        .unwrap_or_else(|e| panic!("open: {e}"));
    journal
        .append(&observation("z"), 100)
        .await
        .unwrap_or_else(|e| panic!("append: {e}"));
    let target = serde_json::from_value(endpoint.clone()).unwrap_or_else(|e| panic!("target: {e}"));
    let snapshot = journal
        .capture_addresses(&target)
        .await
        .unwrap_or_else(|e| panic!("snapshot: {e}"));
    journal
        .append(&observation("a"), 101)
        .await
        .unwrap_or_else(|e| panic!("append: {e}"));
    let current = journal
        .capture_addresses(&target)
        .await
        .unwrap_or_else(|e| panic!("snapshot: {e}"));
    assert_eq!(snapshot.watermark.sequence, 1);
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(current.watermark.sequence, 2);
    assert_eq!(current.entries.len(), 2);
    assert_eq!(
        current
            .entries
            .first()
            .map(|entry| String::from(entry.address.native_thread_id.clone())),
        Some("a".into())
    );
    let mut cache = lifecycle_observation::AddressSnapshotCache::default();
    let now = std::time::Instant::now();
    let snapshot_id = "00000000-0000-4000-8000-000000000004"
        .to_owned()
        .try_into()
        .unwrap_or_else(|e| panic!("id: {e}"));
    let page = cache
        .insert(lifecycle_observation::AddressSnapshotInputs {
            id: snapshot_id,
            endpoint: target.clone(),
            page_size: 1,
            captured: current,
            now,
            captured_at: "2026-09-05T12:00:00Z"
                .to_owned()
                .try_into()
                .unwrap_or_else(|e| panic!("time: {e}")),
            coverage: lifecycle_observation::ObservationCoverage::default().view(
                &target,
                "2026-09-05T12:00:00Z"
                    .to_owned()
                    .try_into()
                    .unwrap_or_else(|e| panic!("time: {e}")),
            ),
        })
        .unwrap_or_else(|e| panic!("insert: {e}"));
    let cursor = page.next_cursor.unwrap_or_else(|| panic!("cursor"));
    let next = cache
        .page(&target, 1, &cursor, now)
        .unwrap_or_else(|e| panic!("page: {e}"));
    assert_eq!(next.entries.len(), 1);
    assert!(next.next_cursor.is_none());
    assert!(matches!(
        cache.page(
            &target,
            1,
            &cursor,
            now + std::time::Duration::from_secs(60)
        ),
        Err(lifecycle_observation::JournalError::SnapshotExpired)
    ));
    journal.close().await;
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
