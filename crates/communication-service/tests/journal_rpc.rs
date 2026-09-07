use communication_service::{ServiceIdentity, serve_control_connection};
use lifecycle_observation::{LifecycleStore, ObservationJournal};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
    let identity = ServiceIdentity::new(
        service,
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap_or_else(|e| panic!("identity: {e}"))
    .with_endpoints(vec![description])
    .unwrap_or_else(|e| panic!("endpoints: {e}"))
    .with_journal(Arc::clone(&store));
    let (client, server) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let task = tokio::spawn(serve_control_connection(server, identity.clone()));
    let (read, mut write) = client.into_split();
    let mut lines = BufReader::new(read).lines();
    let init = json!({"jsonrpc":"2.0","id":"init","method":"control/initialize","params":{"version":{"major":1,"minor":0},"client":{"name":"test","version":"1"}}});
    write
        .write_all(format!("{init}\n").as_bytes())
        .await
        .unwrap_or_else(|e| panic!("write: {e}"));
    assert!(
        lines
            .next_line()
            .await
            .unwrap_or_else(|e| panic!("read: {e}"))
            .is_some()
    );
    let waiting = json!({"jsonrpc":"2.0","id":"waiting","method":"lifecycleJournal/read","params":{"endpoint":endpoint,"after":{"journalId":journal_id,"sequence":0},"pageSize":100,"waitMilliseconds":30000}});
    let status =
        json!({"jsonrpc":"2.0","id":"status","method":"lifecycleJournal/status","params":{}});
    write
        .write_all(format!("{waiting}\n{status}\n").as_bytes())
        .await
        .unwrap_or_else(|e| panic!("write: {e}"));
    let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap_or_else(|e| panic!("status blocked: {e}"))
        .unwrap_or_else(|e| panic!("read: {e}"))
        .unwrap_or_else(|| panic!("response"));
    let response: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("JSON: {e}"));
    assert_eq!(response["id"], "status");
    let observation=serde_json::from_value(json!({"observedAt":"2026-09-05T12:00:00Z","source":"observerLifecycle","scope":{"endpoint":endpoint,"generation":null,"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"backend"},"change":{"kind":"coverageLost"}})).unwrap_or_else(|e|panic!("observation: {e}"));
    store
        .append(&observation, 100)
        .await
        .unwrap_or_else(|e| panic!("append: {e}"));
    let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap_or_else(|e| panic!("wake timeout: {e}"))
        .unwrap_or_else(|e| panic!("read: {e}"))
        .unwrap_or_else(|| panic!("response"));
    let response: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("JSON: {e}"));
    assert_eq!(response["id"], "waiting");
    assert_eq!(response["result"]["next"]["sequence"], 1);
    let address_request = json!({"jsonrpc":"2.0","id":"addresses","method":"addressBook/list","params":{"endpoint":endpoint,"pageSize":100}});
    write
        .write_all(format!("{address_request}\n").as_bytes())
        .await
        .unwrap_or_else(|e| panic!("write: {e}"));
    let line = lines
        .next_line()
        .await
        .unwrap_or_else(|e| panic!("read: {e}"))
        .unwrap_or_else(|| panic!("response"));
    let addresses: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("JSON: {e}"));
    assert_eq!(addresses["id"], "addresses");
    assert_eq!(addresses["result"]["watermark"]["sequence"], 1);
    assert_eq!(addresses["result"]["coverage"]["state"], "disconnected");
    assert_eq!(addresses["result"]["entries"], json!([]));
    write
        .shutdown()
        .await
        .unwrap_or_else(|e| panic!("close: {e}"));
    task.await
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("serve: {e}"));
    let (client, server) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = communication_client::ControlClient::initialize(client, "typed-journal", "1")
        .await
        .unwrap_or_else(|e| panic!("initialize: {e}"));
    assert!(matches!(
        client
            .journal_status()
            .await
            .unwrap_or_else(|e| panic!("status: {e}")),
        communication_client::JournalStatus::Available { .. }
    ));
    let after = communication_protocol::JournalPosition {
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
        Err(communication_client::ClientError::Rejected {
            code: -32050,
            data: Some(data),
        }) => assert_eq!(data["kind"], "historyExpired"),
        other => panic!("expected expired history, got {other:?}"),
    }
    client
        .close()
        .await
        .unwrap_or_else(|e| panic!("close: {e}"));
    task.await
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("serve: {e}"));
    Arc::try_unwrap(store)
        .unwrap_or_else(|_| panic!("store shared"))
        .close()
        .await;
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
