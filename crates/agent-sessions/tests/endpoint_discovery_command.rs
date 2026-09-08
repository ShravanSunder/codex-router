use std::process::Command;
#[test]
fn address_command_reports_missing_service_without_starting_a_runtime() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "addresses",
            "list",
            "--endpoint",
            "codex-local",
            "--json",
            "--service-directory",
            "/tmp/nonexistent-agent-service-proof/missing",
        ])
        .output()
        .unwrap_or_else(|error| panic!("command: {error}"));
    assert_eq!(output.status.code(), Some(3));
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("JSON: {error}"));
    assert_eq!(value["error"]["kind"], "unavailable");
}
#[test]
fn missing_service_is_a_machine_readable_unavailable_result() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "endpoints",
            "list",
            "--json",
            "--service-directory",
            "/tmp/nonexistent-agent-service-proof/missing",
        ])
        .output()
        .unwrap_or_else(|e| panic!("command: {e}"));
    assert_eq!(output.status.code(), Some(3));
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("JSON: {e}"));
    assert_eq!(value["kind"], "error");
    assert_eq!(value["error"]["kind"], "unavailable");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn executable_discovers_an_isolated_published_service() {
    use communication_service::{LocalControlService, ManifestPublication, ServiceIdentity};
    use std::os::unix::fs::DirBuilderExt;
    let root = std::path::PathBuf::from(format!("/tmp/endpoint-cli-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|e| panic!("directory: {e}"));
    let digest = format!("sha256:{}", "a".repeat(64));
    let journal_id = "00000000-0000-4000-8000-000000000003"
        .to_owned()
        .try_into()
        .unwrap_or_else(|error| panic!("journal identity: {error}"));
    let journal = lifecycle_observation::ObservationJournal::open(
        &root.join("session-registry.sqlite"),
        journal_id,
    )
    .await
    .unwrap_or_else(|error| panic!("journal: {error}"));
    let store = std::sync::Arc::new(lifecycle_observation::LifecycleStore::new(journal));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &digest,
    )
    .unwrap_or_else(|e| panic!("identity: {e}"))
    .with_journal(std::sync::Arc::clone(&store));
    let endpoints = identity.endpoint_directory();
    let listener = LocalControlService::bind(&root.join("control.sock"), identity)
        .unwrap_or_else(|e| panic!("bind: {e}"));
    let manifest=serde_json::from_value(serde_json::json!({"version":1,"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":"00000000-0000-4000-8000-000000000002","control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest})).unwrap_or_else(|e|panic!("manifest: {e}"));
    let publication =
        ManifestPublication::publish(&root, &manifest).unwrap_or_else(|e| panic!("publish: {e}"));
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(listener.run(stop.clone()));
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args(["endpoints", "list", "--json", "--service-directory"])
        .arg(&root)
        .output()
        .await
        .unwrap_or_else(|e| panic!("command: {e}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("JSON: {e}"));
    assert_eq!(result["kind"], "result");
    assert_eq!(result["result"]["endpoints"], serde_json::json!([]));
    assert!(output.stderr.is_empty());
    let endpoint = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "label":"Local fixture", "availability":{"state":"unavailable","observedAt":"2026-09-05T12:00:00Z","reason":"No runtime"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":null}]
    })).unwrap_or_else(|error| panic!("endpoint: {error}"));
    endpoints
        .publish(endpoint)
        .unwrap_or_else(|error| panic!("endpoint publication: {error}"));
    let observation = serde_json::from_value(serde_json::json!({
        "observedAt":"2026-09-05T12:00:00Z","source":"inventoryRead",
        "scope":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":null,"observerId":"00000000-0000-4000-8000-000000000004"},
        "subject":{"kind":"thread","address":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"nativeThreadId":"remembered-thread"}},
        "change":{"kind":"threadDiscovered"}
    })).unwrap_or_else(|error| panic!("observation: {error}"));
    store
        .append(&observation, 100)
        .await
        .unwrap_or_else(|error| panic!("append: {error}"));
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "addresses",
            "list",
            "--endpoint",
            "codex-local",
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await
        .unwrap_or_else(|error| panic!("addresses: {error}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let snapshot: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("snapshot: {error}"));
    assert_eq!(
        snapshot["result"]["entries"][0]["address"]["nativeThreadId"],
        "remembered-thread"
    );
    assert_eq!(
        snapshot["result"]["entries"][0]["lastStatus"],
        serde_json::Value::Null
    );
    assert_eq!(snapshot["result"]["coverage"]["state"], "initializing");
    assert_eq!(snapshot["result"]["watermark"]["sequence"], 1);
    assert_eq!(snapshot["result"]["nextCursor"], serde_json::Value::Null);
    let journal_id = snapshot["result"]["watermark"]["journalId"]
        .as_str()
        .unwrap_or_else(|| panic!("journal ID"));
    let replay = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "journal",
            "read",
            "--endpoint",
            "codex-local",
            "--journal-id",
            journal_id,
            "--after",
            "0",
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await
        .unwrap_or_else(|error| panic!("journal replay: {error}"));
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stdout)
    );
    let replay: serde_json::Value = serde_json::from_slice(&replay.stdout)
        .unwrap_or_else(|error| panic!("journal JSON: {error}"));
    assert_eq!(
        replay["result"]["records"][0]["change"]["kind"],
        "threadDiscovered"
    );
    assert_eq!(replay["result"]["next"]["sequence"], 1);
    let changed = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "journal",
            "read",
            "--endpoint",
            "codex-local",
            "--journal-id",
            "00000000-0000-4000-8000-000000000099",
            "--after",
            "0",
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await
        .unwrap_or_else(|error| panic!("changed journal: {error}"));
    assert_eq!(changed.status.code(), Some(4));
    let changed: serde_json::Value = serde_json::from_slice(&changed.stdout)
        .unwrap_or_else(|error| panic!("error JSON: {error}"));
    assert_eq!(changed["error"]["data"]["kind"], "journalChanged");
    assert_eq!(changed["error"]["data"]["current"]["journalId"], journal_id);
    stop.cancel();
    task.await
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("listener: {e}"));
    drop(publication);
    std::sync::Arc::try_unwrap(store)
        .unwrap_or_else(|_| panic!("store retained"))
        .close()
        .await;
    std::fs::remove_file(root.join("session-registry.sqlite"))
        .unwrap_or_else(|error| panic!("journal cleanup: {error}"));
    std::fs::remove_dir(root).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
