use communication_protocol::LifecycleObservation;
use lifecycle_observation::ObservationJournal;
use serde_json::json;

#[tokio::test]
async fn lifecycle_disposition_is_committed_with_journal_and_survives_reopen() {
    let path = std::path::PathBuf::from(format!(
        "/tmp/address-projection-{}.sqlite",
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
    let address = json!({"endpoint":endpoint,"nativeThreadId":"t"});
    let mut journal = ObservationJournal::open(&path, id())
        .await
        .unwrap_or_else(|e| panic!("open: {e}"));
    for change in [
        json!({"kind":"threadDiscovered"}),
        json!({"kind":"threadArchived"}),
        json!({"kind":"threadUnarchived"}),
        json!({"kind":"threadClosed"}),
        json!({"kind":"threadDeleted"}),
        json!({"kind":"threadDiscovered"}),
    ] {
        let observation:LifecycleObservation=serde_json::from_value(json!({"observedAt":"2026-09-05T12:00:00Z","source":"nativeNotification","scope":{"endpoint":endpoint,"generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000003","generation":1},"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"thread","address":address},"change":change})).unwrap_or_else(|e|panic!("observation: {e}"));
        journal
            .append(&observation, 100)
            .await
            .unwrap_or_else(|e| panic!("append: {e}"));
    }
    journal.close().await;
    let mut journal = ObservationJournal::open(&path, id())
        .await
        .unwrap_or_else(|e| panic!("reopen: {e}"));
    let entry = journal
        .address_entry(&serde_json::from_value(address).unwrap_or_else(|e| panic!("address: {e}")))
        .await
        .unwrap_or_else(|e| panic!("entry: {e}"))
        .unwrap_or_else(|| panic!("missing entry"));
    let entry = serde_json::to_value(entry).unwrap_or_else(|e| panic!("serialize: {e}"));
    assert_eq!(entry["disposition"]["existence"], "deleted");
    assert_eq!(entry["disposition"]["archive"], "unarchived");
    assert_eq!(entry["disposition"]["lastClose"]["sequence"], 4);
    assert_eq!(entry["lastObservation"]["sequence"], 6);
    assert!(entry["statusScope"].is_null());
    journal.close().await;
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
