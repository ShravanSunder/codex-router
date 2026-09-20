use serde_json::{Value, json};
use std::os::unix::fs::DirBuilderExt;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn approval_list_rejection_preserves_rejected_kind_and_exit_four() {
    let root = std::path::PathBuf::from(format!("/tmp/approval-list-cli-{}", uuid::Uuid::now_v7()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("private fixture directory");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("service identity");
    let control =
        collaboration_service::LocalControlService::bind(&root.join("control.sock"), identity)
            .expect("control listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("manifest");
    let publication = collaboration_service::ManifestPublication::publish(&root, &manifest)
        .expect("manifest publication");
    let stop = CancellationToken::new();
    let service = tokio::spawn(control.run(stop.clone()));
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["approval", "list", "--json", "--service-directory"])
        .arg(&root)
        .output()
        .await
        .expect("approval list output");
    stop.cancel();
    service
        .await
        .expect("service join")
        .expect("service shutdown");
    drop(publication);
    std::fs::remove_dir(&root).expect("fixture cleanup");

    assert_eq!(
        output.status.code(),
        Some(4),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let record: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
    assert_eq!(record["kind"], "error");
    assert_eq!(record["error"]["kind"], "rejected");
    assert_eq!(record["error"]["effect"], "none");
}
