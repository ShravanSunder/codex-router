use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[tokio::test]
async fn native_bridge_preserves_wire_payload_through_public_discovery() {
    use communication_service::{LocalControlService, ManifestPublication, ServiceIdentity};
    use std::os::unix::fs::DirBuilderExt;
    let root = std::path::PathBuf::from(format!("/tmp/native-cli-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|e| panic!("directory: {e}"));
    let digest = format!("sha256:{}", "a".repeat(64));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &digest,
    )
    .unwrap_or_else(|e| panic!("identity: {e}"));
    let native_path = root.join("native.sock");
    let native_listener =
        tokio::net::UnixListener::bind(&native_path).unwrap_or_else(|e| panic!("native bind: {e}"));
    let native_task = tokio::spawn(async move {
        let (stream, _) = native_listener
            .accept()
            .await
            .unwrap_or_else(|e| panic!("accept: {e}"));
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .unwrap_or_else(|e| panic!("upgrade: {e}"));
        let message = socket
            .next()
            .await
            .unwrap_or_else(|| panic!("message"))
            .unwrap_or_else(|e| panic!("frame: {e}"));
        assert_eq!(
            message.to_text().unwrap_or_else(|e| panic!("text: {e}")),
            "{ \"id\":9223372036854775807,\"method\":\"thread/read\" }"
        );
        socket
            .send(message)
            .await
            .unwrap_or_else(|e| panic!("echo: {e}"));
        let _close = socket.next().await;
    });
    let endpoint=serde_json::from_value(serde_json::json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"label":"fixture","availability":{"state":"available","observedAt":"2026-09-05T12:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]})).unwrap_or_else(|e|panic!("endpoint: {e}"));
    let identity = identity
        .with_endpoints(vec![endpoint])
        .unwrap_or_else(|e| panic!("register: {e}"));
    let listener = LocalControlService::bind(&root.join("control.sock"), identity)
        .unwrap_or_else(|e| panic!("bind: {e}"));
    let manifest=serde_json::from_value(serde_json::json!({"version":1,"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":"00000000-0000-4000-8000-000000000002","control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest})).unwrap_or_else(|e|panic!("manifest: {e}"));
    let publication =
        ManifestPublication::publish(&root, &manifest).unwrap_or_else(|e| panic!("publish: {e}"));
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(listener.run(stop.clone()));
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args(["native", "--endpoint", "codex-local", "--service-directory"])
        .arg(&root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|e| panic!("spawn: {e}"));
    let mut input = child.stdin.take().unwrap_or_else(|| panic!("stdin"));
    let output = child.stdout.take().unwrap_or_else(|| panic!("stdout"));
    let payload = "{ \"id\":9223372036854775807,\"method\":\"thread/read\" }";
    input
        .write_all(format!("{payload}\n").as_bytes())
        .await
        .unwrap_or_else(|e| panic!("write: {e}"));
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        BufReader::new(output).lines().next_line(),
    )
    .await
    .unwrap_or_else(|e| panic!("timeout: {e}"))
    .unwrap_or_else(|e| panic!("read: {e}"));
    assert_eq!(response.as_deref(), Some(payload));
    drop(input);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
            .await
            .unwrap_or_else(|e| panic!("exit timeout: {e}"))
            .unwrap_or_else(|e| panic!("exit: {e}"))
            .success()
    );
    native_task
        .await
        .unwrap_or_else(|e| panic!("native task: {e}"));
    std::fs::remove_file(native_path).unwrap_or_else(|e| panic!("native cleanup: {e}"));
    stop.cancel();
    task.await
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("listener: {e}"));
    drop(publication);
    std::fs::remove_dir(root).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
