use std::{os::unix::fs::PermissionsExt, sync::Arc};

use claude_code_peer_messaging::ClaudeCodeSessionRegistry;
use collaboration_client::protocol::{
    EndpointDescription, ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess,
    SessionRef,
};
use collaboration_service::{
    LocalControlService, ManifestPublication, ProviderOperationStore, ProviderSessionRecord,
    ServiceIdentity,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cli_dispatches_provider_session_list_by_endpoint_channel() {
    let root = tempfile::Builder::new()
        .prefix("provider-session-cli-")
        .tempdir_in("/tmp")
        .expect("private service directory");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private directory mode");
    let registry_directory = root.path().join("claude-fixture-registry");
    std::fs::create_dir(&registry_directory).expect("fixture registry directory");
    let process_id = std::process::id();
    std::fs::write(
        registry_directory.join(format!("{process_id}.json")),
        json!({
            "pid":process_id,"sessionId":"interactive-session","peerProtocol":1,
            "messagingSocketPath":root.path().join("secret-peer.sock"),
            "cwd":root.path(),"name":"Interactive fixture","status":"idle",
        "startedAt":1_790_162_100_123_i64,"updatedAt":1_790_162_494_441_i64,"kind":"interactive","entrypoint":"cli"
        })
        .to_string(),
    )
    .expect("fixture registry record");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"claude-local"},"sessionId":"provider-session"
    })).expect("target");
    let creator: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"creator"
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
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: creator.clone().into(),
            approver: creator.into(),
            updated_at_ms: 3_000,
        })
        .await
        .expect("record");
    let endpoint: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Claude fixture",
        "availability":{"state":"available","observedAt":"2026-09-26T00:00:00Z"},
        "channels":[{"kind":"externalProvider","transport":"stdioAcp",
            "bindingId":"fixture-binding","bindingGeneration":7,
            "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
            "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
    }))
    .expect("endpoint");
    let native_endpoint: EndpointDescription = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "label":"Codex fixture",
        "availability":{"state":"available","observedAt":"2026-09-26T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock",
            "schemaDigest":null,"generation":null}]
    }))
    .expect("native endpoint");
    let identity = ServiceIdentity::new(service_id, epoch)
        .expect("identity")
        .with_endpoints(vec![endpoint, native_endpoint])
        .expect("endpoint inventory")
        .with_provider_operation_store(store)
        .with_claude_code_sessions(Arc::new(ClaudeCodeSessionRegistry::new(registry_directory)));
    let control = LocalControlService::bind(&root.path().join("control.sock"), identity)
        .expect("control listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("manifest");
    let _publication = ManifestPublication::publish(root.path(), &manifest).expect("publish");
    let stop = CancellationToken::new();
    let server = tokio::spawn(control.run(stop.clone()));
    let run = |endpoint_id: &str, view: &str, source: Option<&str>| {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
        command
            .args([
                "sessions",
                "list",
                "--endpoint",
                endpoint_id,
                "--view",
                view,
                "--any",
                "--json",
                "--service-directory",
            ])
            .arg(root.path());
        if let Some(source) = source {
            command.args(["--source", source]);
        }
        command
    };
    let output = run("claude-local", "stored", None)
        .output()
        .await
        .expect("provider CLI");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).expect("CLI result");
    assert_eq!(
        result["result"]["page"]["records"][0]["target"],
        json!(target)
    );
    assert_eq!(result["result"]["page"]["records"][0]["state"], "unloaded");
    assert_eq!(
        result["result"]["page"]["records"][0]["origin"],
        "hostedProvider"
    );
    let interactive = run("claude-local", "active", Some("interactive"))
        .output()
        .await
        .expect("interactive CLI");
    assert_eq!(
        interactive.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&interactive.stderr)
    );
    let interactive_result: Value =
        serde_json::from_slice(&interactive.stdout).expect("interactive JSON");
    assert_eq!(
        interactive_result["result"]["page"]["records"][0]["origin"],
        "claudeCodeInteractive"
    );
    assert_eq!(
        interactive_result["result"]["page"]["records"][0]["target"]["sessionId"],
        "interactive-session"
    );
    assert_eq!(
        interactive_result["result"]["page"]["records"][0]["updatedAt"],
        1_790_162_494
    );
    assert_eq!(interactive_result["result"]["page"]["skippedRecords"], 0);
    assert!(!String::from_utf8_lossy(&interactive.stdout).contains("secret-peer.sock"));
    let rejected = run("claude-local", "stored", Some("interactive"))
        .output()
        .await
        .expect("source rejection");
    assert_eq!(rejected.status.code(), Some(4));
    let error: Value = serde_json::from_slice(&rejected.stdout).expect("error JSON");
    assert_eq!(error["error"]["code"], -32050);
    assert!(
        error["error"]["data"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--view active"))
    );
    let missing_native_source = run("codex-local", "stored", None)
        .output()
        .await
        .expect("native source validation");
    assert_eq!(missing_native_source.status.code(), Some(2));
    let error: Value =
        serde_json::from_slice(&missing_native_source.stdout).expect("native error JSON");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--source is required for Codex sessions"))
    );
    stop.cancel();
    server.await.expect("server task").expect("server");
}
