//! Real public-client boundary: inventory reads never resume or submit work.
use super::*;

#[test]
fn runtime_picker_empty_native_names_preserve_fallback_titles_and_search() {
    for (name, fallback, expected) in [
        (Some(""), "Inventory title", "Inventory title"),
        (Some(""), "", "runtime-id"),
        (Some("Native title"), "unused", "Native title"),
        (None, "Inventory title", "Inventory title"),
    ] {
        let row = runtime_record("runtime-id", fallback, "/repo", &json!({"name":name}));
        assert_eq!(row.title, expected);
        assert_eq!(row.full_title, expected);
        assert!(row.matches_search(
            &collaboration_client::session_catalog::SessionSearchExpression::parse(expected)
        ));
    }
}

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
use std::os::unix::fs::PermissionsExt;
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
fn inventory_with_provider() -> Value {
    let mut entries = inventory();
    entries["endpoints"]
        .as_array_mut()
        .expect("endpoints")
        .push(json!({
            "endpoint":{"serviceId":SERVICE,"endpointId":"claude-local"},
            "label":"Claude fixture",
            "availability":{"state":"available","observedAt":"2026-09-07T00:00:00Z"},
            "channels":[{"kind":"externalProvider","transport":"stdioAcp",
                "bindingId":"fixture-binding","bindingGeneration":7,
                "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
                "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
        }));
    entries
}
fn page(id: &str, status: Value, cursor: Value) -> Value {
    json!({
        "endpoint":endpoint(),"generation":generation(),"observedAt":"2026-09-07T00:00:00Z",
        "sessions":[{"target":{"endpoint":endpoint(),"sessionId":id},"name":null,"title":id,"source":"interactive","gitBranch":null,"workingDirectory":"/repo",
            "observation":{"kind":"runtime","status":status,"turnId":null},"model":"gpt-5.6-sol","reasoningEffort":"medium","idleSeconds":0}],"nextCursor":cursor
    })
}
fn inspected(id: &str) -> Value {
    json!({"target":{"endpoint":endpoint(),"sessionId":id},"generation":generation(),
    "effectiveAccess":null,
    "settingsObservation":{"kind":"unavailable","reason":"threadReadOmitsSettings"},
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
    let (_, rows) = load_runtime_records(&mut client, &[], false).await.unwrap();
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
    assert_eq!(requests[1]["params"]["includeEmptySessions"], false);
    assert_eq!(requests[3]["params"]["cursor"], "next-page");
}

#[tokio::test]
async fn runtime_picker_passes_the_empty_session_opt_in_to_inventory() {
    let (mut client, peer) = connect_fixture(vec![
        ("endpoint/list", inventory()),
        (
            "codex/sessionList",
            page("empty-thread", json!({"type":"idle"}), Value::Null),
        ),
        ("codex/sessionInspect", inspected("empty-thread")),
        ("endpoint/list", inventory()),
    ])
    .await;

    let (_, records) = load_runtime_records(&mut client, &[], true)
        .await
        .expect("explicitly opted-in inventory");
    client.close().await.unwrap();
    let requests = peer.await.unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, "empty-thread");
    assert_eq!(requests[1]["params"]["includeEmptySessions"], true);
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
    assert!(load_runtime_records(&mut client, &[], false).await.is_err());
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
    assert!(load_runtime_records(&mut client, &[], false).await.is_err());
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
    let (_, rows) = load_runtime_records(&mut client, &[stored], false)
        .await
        .unwrap();
    client.close().await.unwrap();
    peer.await.unwrap();
    // Assert
    assert_eq!(rows[0].title, "Stored title");
    assert_eq!(rows[0].conversation.snippets, vec!["Retained preview"]);
    assert_eq!(rows[0].runtime_status, PickerRuntimeStatus::Active);
}

#[tokio::test]
async fn provider_inventory_reaches_picker_as_a_qualified_read_only_row() {
    let provider_page = json!({
        "endpoint":{"serviceId":SERVICE,"endpointId":"claude-local"},
        "observedAt":"2026-09-07T00:00:00Z",
        "sessions":[{
            "origin":"hostedProvider",
            "target":{"endpoint":{"serviceId":SERVICE,"endpointId":"claude-local"},"sessionId":"first"},
            "workingDirectory":"/repo","updatedAt":3,"state":"requiresAction",
            "approver":{"kind":"human","humanId":"owner"},
            "createdBy":{"endpoint":endpoint(),"sessionId":"creator"}
        }],"nextCursor":null
    });
    let (mut client, peer) = connect_fixture(vec![
        ("endpoint/list", inventory_with_provider()),
        ("provider/sessionList", provider_page),
    ])
    .await;
    let (native_endpoint, rows) = load_provider_records(&mut client)
        .await
        .expect("provider rows");
    client.close().await.expect("close");
    let requests = peer.await.expect("peer");
    assert_eq!(
        native_endpoint.expect("native endpoint"),
        serde_json::from_value(endpoint()).unwrap()
    );
    assert_eq!(rows.len(), 1);
    assert!(matches!(
        rows[0].identity,
        SessionPickerIdentity::HostedProvider(_)
    ));
    assert!(rows[0].title.contains("Claude fixture"));
    assert_eq!(rows[0].runtime_status, PickerRuntimeStatus::Blocked);
    assert_eq!(requests[1]["params"]["source"], "all");
}

#[tokio::test]
async fn hosted_refresh_keeps_equal_provider_and_codex_ids_as_two_rows() {
    use collaboration_client::protocol::{
        EndpointDescription, ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess,
        SessionRef,
    };
    use collaboration_service::{
        LocalControlService, ManifestPublication, ProviderOperationStore, ProviderSessionRecord,
        ServiceIdentity,
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let root = tempfile::Builder::new()
        .prefix("provider-picker-")
        .tempdir_in("/tmp")
        .expect("service root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private root");
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":SERVICE,"endpointId":"claude-local"},"sessionId":"shared-id"
    }))
    .expect("provider target");
    let creator: SessionRef = serde_json::from_value(json!({
        "endpoint":endpoint(),"sessionId":"creator"
    }))
    .expect("creator");
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from("/repo/project-a".to_owned())
                .expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: creator.clone().into(),
            approver: creator.into(),
            updated_at_ms: 3_000,
        })
        .await
        .expect("provider record");
    let descriptions: Vec<EndpointDescription> = inventory_with_provider()["endpoints"]
        .as_array()
        .expect("descriptions")
        .iter()
        .cloned()
        .map(|entry| serde_json::from_value(entry).expect("description"))
        .collect();
    let digest = format!("sha256:{}", "a".repeat(64));
    let identity = ServiceIdentity::new(SERVICE, EPOCH, &digest)
        .expect("identity")
        .with_endpoints(descriptions)
        .expect("endpoints")
        .with_provider_operation_store(store);
    let control =
        LocalControlService::bind(&root.path().join("control.sock"), identity).expect("listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":SERVICE,"serviceEpoch":EPOCH,
        "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("manifest");
    let _publication = ManifestPublication::publish(root.path(), &manifest).expect("publication");
    let stop = CancellationToken::new();
    let server = tokio::spawn(control.run(stop.clone()));
    let codex = runtime_record(
        "shared-id",
        "Codex row",
        "/repo/project-a",
        &json!({"source":"cli"}),
    );
    let rows = PickerRuntimeInventory::default()
        .refresh(Some(root.path()), vec![codex], false)
        .await
        .records;
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .any(|row| matches!(row.identity, SessionPickerIdentity::HostedCodex(_)))
    );
    assert!(
        rows.iter()
            .any(|row| matches!(row.identity, SessionPickerIdentity::HostedProvider(_)))
    );
    stop.cancel();
    server.await.expect("server task").expect("server");
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
    let snapshot = inventory.refresh(Some(&missing), vec![], false).await;
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

#[test]
fn claude_interactive_picker_timestamps_convert_seconds_to_milliseconds_once() {
    let summary: collaboration_client::protocol::ProviderSessionSummary =
        serde_json::from_value(json!({
            "origin":"claudeCodeInteractive",
            "target":{"endpoint":{"serviceId":SERVICE,"endpointId":"claude-local"},"sessionId":"interactive-one"},
            "name":"Interactive fixture","workingDirectory":"/repo",
            "status":"idle","startedAt":1_790_162_100,"updatedAt":1_790_162_494,
            "statusUpdatedAt":1_790_162_494,"kind":"interactive","entrypoint":"cli"
        }))
        .expect("interactive summary");
    let row = SessionPickerRecord::from_provider_summary(&summary, "Claude fixture");
    assert_eq!(row.created_at_ms, Some(1_790_162_100_000));
    assert_eq!(row.recency_at_ms, Some(1_790_162_494_000));
    assert_eq!(row.title, "Interactive fixture");
}

#[tokio::test]
async fn unavailable_service_clears_remembered_provider_state() {
    let summary: collaboration_client::protocol::ProviderSessionSummary =
        serde_json::from_value(json!({
            "origin":"hostedProvider",
            "target":{"endpoint":{"serviceId":SERVICE,"endpointId":"claude-local"},"sessionId":"provider-one"},
            "workingDirectory":"/repo","updatedAt":3,"state":"requiresAction",
            "approver":{"kind":"human","humanId":"owner"},
            "createdBy":{"endpoint":endpoint(),"sessionId":"creator"}
        }))
        .expect("provider summary");
    let row = SessionPickerRecord::from_provider_summary(&summary, "Claude fixture");
    let mut inventory = PickerRuntimeInventory {
        remembered: vec![row],
    };
    let missing =
        std::env::temp_dir().join(format!("missing-provider-picker-{}", std::process::id()));
    let snapshot = inventory.refresh(Some(&missing), vec![], false).await;
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(snapshot.records[0].provider_state, None);
    assert_eq!(
        snapshot.records[0].runtime_status,
        PickerRuntimeStatus::Unknown
    );
}

#[tokio::test]
async fn local_picker_never_reuses_provider_rows() {
    let summary: collaboration_client::protocol::ProviderSessionSummary =
        serde_json::from_value(json!({
            "origin":"hostedProvider",
            "target":{"endpoint":{"serviceId":SERVICE,"endpointId":"claude-local"},"sessionId":"shared-id"},
            "workingDirectory":"/repo","updatedAt":3,"state":"idle",
            "approver":{"kind":"human","humanId":"owner"},
            "createdBy":{"endpoint":endpoint(),"sessionId":"creator"}
        }))
        .expect("provider summary");
    let provider = SessionPickerRecord::from_provider_summary(&summary, "Claude fixture");
    let codex = runtime_record("shared-id", "Codex row", "/repo", &json!({"source":"cli"}));
    let mut inventory = PickerRuntimeInventory {
        remembered: vec![provider],
    };
    let snapshot = inventory.refresh(None, vec![codex], false).await;
    assert_eq!(snapshot.records.len(), 1);
    assert!(matches!(
        snapshot.records[0].identity,
        SessionPickerIdentity::LocalCodex(_)
    ));
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

    let snapshot = inventory.refresh(None, vec![], false).await;
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

#[test]
fn hosted_display_attribution_is_separate_from_actual_runtime_observation() {
    let local = runtime_record("legacy-id", "Local catalog", "/owned/project", &json!({}));
    assert_eq!(
        local.provenance,
        super::super::SessionRowProvenance::LocalHomeCatalog
    );
    let endpoint: EndpointRef = serde_json::from_value(endpoint()).unwrap();
    let attributed = local.with_hosted_codex(&endpoint);
    assert_eq!(
        attributed.provenance,
        super::super::SessionRowProvenance::DefaultAttributed
    );
    assert!(matches!(
        attributed.identity,
        SessionPickerIdentity::HostedCodex(_)
    ));
}
