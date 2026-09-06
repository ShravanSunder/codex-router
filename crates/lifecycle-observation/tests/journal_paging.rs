use communication_protocol::{EndpointRef, LifecycleObservation};
use lifecycle_observation::{JournalError, JournalPosition, ObservationJournal};
use serde_json::json;
#[tokio::test]
async fn filtered_scan_advances_over_other_endpoints_and_checks_identity() {
    let path =
        std::path::PathBuf::from(format!("/tmp/journal-paging-{}.sqlite", std::process::id()));
    let id = "00000000-0000-4000-8000-000000000001"
        .to_owned()
        .try_into()
        .unwrap_or_else(|e| panic!("id: {e}"));
    let mut journal = ObservationJournal::open(&path, id)
        .await
        .unwrap_or_else(|e| panic!("open: {e}"));
    let target: EndpointRef = serde_json::from_value(
        json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"}),
    )
    .unwrap_or_else(|e| panic!("target: {e}"));
    for endpoint in ["codex-local", "codex-other", "codex-other"] {
        let observation:LifecycleObservation=serde_json::from_value(json!({"observedAt":"2026-09-05T12:00:00Z","source":"observerLifecycle","scope":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":endpoint},"generation":null,"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"backend"},"change":{"kind":"coverageLost"}})).unwrap_or_else(|e|panic!("observation: {e}"));
        journal
            .append(&observation, 100)
            .await
            .unwrap_or_else(|e| panic!("append: {e}"));
    }
    let page = journal
        .read_page(
            &target,
            JournalPosition {
                journal_id: journal.journal_id().clone(),
                sequence: 0,
            },
            100,
        )
        .await
        .unwrap_or_else(|e| panic!("page: {e}"));
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.next.sequence, 3);
    assert!(page.caught_up);
    assert_eq!(page.bounds.last_sequence, 3);
    assert!(matches!(
        journal
            .read_page(
                &target,
                JournalPosition {
                    journal_id: "00000000-0000-4000-8000-000000000099"
                        .to_owned()
                        .try_into()
                        .unwrap_or_else(|e| panic!("id: {e}")),
                    sequence: 0
                },
                100
            )
            .await,
        Err(JournalError::JournalChanged)
    ));
    journal.close().await;
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
