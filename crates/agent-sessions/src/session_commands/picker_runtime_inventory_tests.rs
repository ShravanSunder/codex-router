//! Real public-client boundary: inventory reads never resume or submit work.
use super::*;

#[test]
fn runtime_picker_preserves_native_subagent_classification() {
    let row = runtime_record(
        "child",
        "Helper",
        "/repo",
        &json!({
            "source":"vscode", "threadSource":"subagent", "parentThreadId":"parent"
        }),
    );
    assert_eq!(row.thread_source.as_deref(), Some("subagent"));
}
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
const EPOCH: &str = "00000000-0000-4000-8000-000000000002";

fn endpoint() -> Value {
    json!({"serviceId":SERVICE,"endpointId":"codex-local"})
}
fn generation() -> Value {
    json!({"serviceEpoch":EPOCH,"generation":1})
}
fn inventory() -> Value {
    json!({"serviceEpoch":EPOCH,"sequence":0,"endpoints":[{
        "endpoint":endpoint(),"label":"Debug Codex","availability":{"state":"available","observedAt":"2026-09-07T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":generation()}]
    }]})
}
fn page(id: &str, status: Value, cursor: Value) -> Value {
    json!({
        "endpoint":endpoint(),"generation":generation(),"observedAt":"2026-09-07T00:00:00Z",
        "sessions":[{"target":{"endpoint":endpoint(),"sessionId":id},"title":id,"workingDirectory":"/repo",
            "observation":{"kind":"runtime","status":status,"turnId":null}}],"nextCursor":cursor
    })
}
fn inspected(id: &str) -> Value {
    json!({"target":{"endpoint":endpoint(),"sessionId":id},"generation":generation(),
    "thread":{"id":id,"name":format!("Live {id}"),"cwd":"/repo","modelProvider":"debug-provider","createdAt":100,"updatedAt":200,
    "gitInfo":{"branch":"feature/live","originUrl":"https://example.invalid/repo.git"}}})
}
async fn connect_fixture(
    steps: Vec<(&'static str, Value)>,
) -> (ControlClient, tokio::task::JoinHandle<Vec<Value>>) {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let task = tokio::spawn(async move {
        let (read, mut write) = server.into_split();
        let mut lines = BufReader::new(read).lines();
        let request: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(request["method"], "control/initialize");
        let result = json!({"version":{"major":1,"minor":0},"serviceId":SERVICE,"serviceEpoch":EPOCH,"controlSchemaDigest":format!("sha256:{}","a".repeat(64))});
        write
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":request["id"],"result":result})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut observed = Vec::new();
        for (method, result) in steps {
            let line = lines
                .next_line()
                .await
                .unwrap()
                .expect("expected read operation");
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], method);
            write
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":request["id"],"result":result})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            observed.push(request);
        }
        assert!(
            lines.next_line().await.unwrap().is_none(),
            "no mutation or retry allowed"
        );
        observed
    });
    (
        ControlClient::initialize(client, "picker-proof", "1")
            .await
            .unwrap(),
        task,
    )
}

#[tokio::test]
async fn paged_runtime_only_threads_include_metadata_and_blocked_status_without_mutations() {
    // Arrange: neither live thread exists in the stored catalog.
    let steps = vec![
        ("endpoint/list", inventory()),
        (
            "codex/sessionList",
            page("first", json!({"type":"idle"}), json!("next-page")),
        ),
        ("codex/sessionInspect", inspected("first")),
        (
            "codex/sessionList",
            page(
                "second",
                json!({"type":"active","activeFlags":["waitingOnApproval"]}),
                Value::Null,
            ),
        ),
        ("codex/sessionInspect", inspected("second")),
        ("endpoint/list", inventory()),
    ];
    let (mut client, peer) = connect_fixture(steps).await;
    // Act
    let rows = load_runtime_records(&mut client, &[]).await.unwrap();
    client.close().await.unwrap();
    let requests = peer.await.unwrap();
    // Assert
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].title, "Live first");
    assert_eq!(rows[0].branch, "feature/live");
    assert_eq!(
        rows[0].runtime_status,
        crate::picker_runtime_status::PickerRuntimeStatus::Idle
    );
    assert_eq!(
        rows[1].runtime_status,
        crate::picker_runtime_status::PickerRuntimeStatus::Blocked
    );
    assert_eq!(requests[1]["params"]["view"], "loaded");
    assert_eq!(requests[3]["params"]["cursor"], "next-page");
}

#[tokio::test]
async fn replacement_during_refresh_discards_the_entire_runtime_result() {
    // Arrange: the endpoint changes generation after its rows are read.
    let mut replacement = inventory();
    replacement["endpoints"][0]["channels"][0]["generation"]["generation"] = json!(2);
    let (mut client, peer) = connect_fixture(vec![
        ("endpoint/list", inventory()),
        (
            "codex/sessionList",
            page("first", json!({"type":"idle"}), Value::Null),
        ),
        ("codex/sessionInspect", inspected("first")),
        ("endpoint/list", replacement),
    ])
    .await;
    // Act / Assert: no mixture of generations may become live picker state.
    assert!(load_runtime_records(&mut client, &[]).await.is_err());
    client.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test]
async fn unavailable_endpoint_does_not_attempt_to_load_or_inspect_threads() {
    // Arrange: a reachable service whose backend is unavailable.
    let mut unavailable = inventory();
    unavailable["endpoints"][0]["availability"] = json!({"state":"unprobed"});
    let (mut client, peer) = connect_fixture(vec![("endpoint/list", unavailable)]).await;
    // Act / Assert
    assert!(load_runtime_records(&mut client, &[]).await.is_err());
    client.close().await.unwrap();
    assert_eq!(peer.await.unwrap().len(), 1);
}

#[tokio::test]
async fn stored_metadata_is_preserved_while_runtime_status_is_refreshed() {
    // Arrange: native inventory only has a sparse title; stored history has a richer label.
    let mut stored = runtime_record(
        "first",
        "Stored title",
        "/repo",
        &json!({"name":"Stored title"}),
    );
    stored.conversation.snippets = vec!["Retained preview".into()];
    let (mut client, peer) = connect_fixture(vec![
        ("endpoint/list", inventory()),
        (
            "codex/sessionList",
            page(
                "first",
                json!({"type":"active","activeFlags":[]}),
                Value::Null,
            ),
        ),
        ("endpoint/list", inventory()),
    ])
    .await;
    // Act
    let rows = load_runtime_records(&mut client, &[stored]).await.unwrap();
    client.close().await.unwrap();
    peer.await.unwrap();
    // Assert
    assert_eq!(rows[0].title, "Stored title");
    assert_eq!(rows[0].conversation.snippets, vec!["Retained preview"]);
    assert_eq!(rows[0].runtime_status, PickerRuntimeStatus::Active);
}

#[tokio::test]
async fn unavailable_service_retains_remembered_rows_without_stale_active_claims() {
    // Arrange: an earlier successful runtime-only row; now the service cannot be reached.
    let mut row = runtime_record("remembered", "Remembered", "/repo", &json!({}));
    row.runtime_status = PickerRuntimeStatus::Active;
    let mut inventory = PickerRuntimeInventory {
        remembered: vec![row],
    };
    // Act
    let missing =
        std::env::temp_dir().join(format!("missing-picker-service-{}", std::process::id()));
    let snapshot = inventory.refresh(Some(&missing), vec![]).await;
    assert_eq!(
        snapshot.runtime_coverage,
        crate::picker_runtime_status::PickerRuntimeCoverage::Unavailable
    );
    let rows = snapshot.records;
    // Assert: remembered identity remains visible in All, but cannot match Active or Idle.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id, "remembered");
    assert_eq!(rows[0].runtime_status, PickerRuntimeStatus::Unknown);
}

#[tokio::test]
async fn remembered_runtime_rows_refresh_age_labels_from_their_timestamps() {
    let mut row = runtime_record("remembered", "Remembered", "/repo", &json!({}));
    row.created_at_ms = Some(0);
    row.recency_at_ms = Some(0);
    row.created = "now".into();
    row.recency = "now".into();
    let mut inventory = PickerRuntimeInventory {
        remembered: vec![row],
    };

    let snapshot = inventory.refresh(None, vec![]).await;
    assert_eq!(
        snapshot.runtime_coverage,
        crate::picker_runtime_status::PickerRuntimeCoverage::LocalOnly
    );
    let rows = snapshot.records;

    assert!(rows[0].created.ends_with(" ago"));
    assert!(rows[0].recency.ends_with(" ago"));
    assert_eq!(rows[0].created_at_ms, Some(0));
    assert_eq!(rows[0].recency_at_ms, Some(0));
}

#[test]
fn ephemeral_system_runtime_record_preserves_system_classification() {
    let row = runtime_record(
        "system-record",
        "",
        "/repo/project-a",
        &json!({
            "source":"vscode", "threadSource":"system", "ephemeral":true,
            "parentThreadId":null, "name":null, "path":null
        }),
    );
    assert_eq!(row.thread_source.as_deref(), Some("system"));
    assert_eq!(row.source.as_deref(), Some("vscode"));
}
