use std::{os::unix::fs::PermissionsExt, sync::Arc};

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
    let identity = ServiceIdentity::new(service_id, epoch, &digest)
        .expect("identity")
        .with_endpoints(vec![endpoint, native_endpoint])
        .expect("endpoint inventory")
        .with_provider_operation_store(store);
    let control = LocalControlService::bind(&root.path().join("control.sock"), identity)
        .expect("control listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("manifest");
    let _publication = ManifestPublication::publish(root.path(), &manifest).expect("publish");
    let stop = CancellationToken::new();
    let server = tokio::spawn(control.run(stop.clone()));
    let run = |endpoint_id: &str, source: Option<&str>| {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
        command
            .args([
                "sessions",
                "list",
                "--endpoint",
                endpoint_id,
                "--view",
                "stored",
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
    let output = run("claude-local", None)
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
    let rejected = run("claude-local", Some("interactive"))
        .output()
        .await
        .expect("source rejection");
    assert_eq!(rejected.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&rejected.stdout).expect("error JSON");
    assert_eq!(error["error"]["code"], -32602);
    let missing_native_source = run("codex-local", None)
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
