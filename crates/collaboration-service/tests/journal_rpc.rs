use collaboration_service::ServiceIdentity;
use lifecycle_observation::{LifecycleStore, ObservationJournal};
use serde_json::json;
use std::{sync::Arc, time::Duration};
#[path = "support/served_api.rs"]
mod served_api;

#[tokio::test]
async fn pending_journal_read_does_not_block_status_and_wakes_after_append() {
    let path = std::path::PathBuf::from(format!("/tmp/journal-rpc-{}.sqlite", std::process::id()));
    let service = "00000000-0000-4000-8000-000000000001";
    let journal = ObservationJournal::open(
        &path,
        service
            .to_owned()
            .try_into()
            .unwrap_or_else(|e| panic!("id: {e}")),
    )
    .await
    .unwrap_or_else(|e| panic!("open: {e}"));
    let journal_id = journal.journal_id().clone();
    let store = Arc::new(LifecycleStore::new(journal));
    let endpoint = json!({"serviceId":service,"endpointId":"codex-local"});
    let description=serde_json::from_value(json!({"endpoint":endpoint,"label":"Codex","availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]})).unwrap_or_else(|e|panic!("description: {e}"));
    let identity = ServiceIdentity::new(service, "00000000-0000-4000-8000-000000000002")
        .unwrap_or_else(|e| panic!("identity: {e}"))
        .with_endpoints(vec![description])
        .unwrap_or_else(|e| panic!("endpoints: {e}"))
        .with_journal(Arc::clone(&store));
    let served = served_api::ServedApi::start(identity)
        .await
        .unwrap_or_else(|e| panic!("serve: {e}"));
    let mut waiting = Box::pin(served.call(
        "journal_read",
        json!({"endpoint":endpoint,"after":{"journalId":journal_id,"sequence":0},"pageSize":100,"waitMilliseconds":30000}),
    ));
    // The status call answers while the read is still waiting.
    let response = tokio::select! {
        early = &mut waiting => panic!("journal read returned before any append: {early:?}"),
        status = tokio::time::timeout(Duration::from_secs(2), served.call("journal_status", json!({}))) => status
            .unwrap_or_else(|e| panic!("status blocked: {e}"))
            .unwrap_or_else(|e| panic!("status: {e}")),
    };
    assert!(response.get("result").is_some(), "{response}");
    let observation=serde_json::from_value(json!({"observedAt":"2026-09-05T12:00:00Z","source":"observerLifecycle","scope":{"endpoint":endpoint,"generation":null,"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"backend"},"change":{"kind":"coverageLost"}})).unwrap_or_else(|e|panic!("observation: {e}"));
    store
        .append(&observation, 100)
        .await
        .unwrap_or_else(|e| panic!("append: {e}"));
    let response = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap_or_else(|e| panic!("wake timeout: {e}"))
        .unwrap_or_else(|e| panic!("read: {e}"));
    assert_eq!(response["result"]["next"]["sequence"], 1);
    let addresses = served
        .call(
            "addresses_list",
            json!({"endpoint":endpoint,"pageSize":100}),
        )
        .await
        .unwrap_or_else(|e| panic!("addresses: {e}"));
    assert_eq!(addresses["result"]["watermark"]["sequence"], 1);
    assert_eq!(addresses["result"]["coverage"]["state"], "disconnected");
    assert_eq!(addresses["result"]["entries"], json!([]));
    let client = served
        .client("typed-journal")
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));
    assert!(matches!(
        client
            .journal_status()
            .await
            .unwrap_or_else(|e| panic!("status: {e}")),
        collaboration_client::JournalStatus::Available { .. }
    ));
    let after = collaboration_protocol::JournalPosition {
        journal_id: journal_id.clone(),
        sequence: 0,
    };
    let target = serde_json::from_value(endpoint).unwrap_or_else(|e| panic!("target: {e}"));
    let page = client
        .read_journal(&target, after.clone(), 100, 0)
        .await
        .unwrap_or_else(|e| panic!("typed read: {e}"));
    assert_eq!(page.records.len(), 1);
    store
        .maintain(40 * 86400)
        .await
        .unwrap_or_else(|e| panic!("retention: {e}"));
    let error = client.read_journal(&target, after, 100, 0).await;
    match error {
        Err(collaboration_client::ClientError::Rejected {
            code: -32050,
            data: Some(data),
        }) => assert_eq!(data["kind"], "historyExpired"),
        other => panic!("expected expired history, got {other:?}"),
    }
    drop(client);
    served.stop().await.unwrap_or_else(|e| panic!("serve: {e}"));
    Arc::try_unwrap(store)
        .unwrap_or_else(|_| panic!("store shared"))
        .close()
        .await;
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
