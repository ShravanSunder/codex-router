use collaboration_client::protocol::ServiceManifest;
use collaboration_protocol::{
    ControlSelector, ControlSocketPath, ControlTransport, MachineLabel, McpSelector, McpTransport,
    SchemaDigest, UuidIdentity,
};
use collaboration_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use tokio_util::sync::CancellationToken;

const SERVICE_ID: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";
const SERVICE_EPOCH: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";

#[tokio::test]
async fn whoami_displays_service_id_and_machine_label_in_text_and_json() {
    let root = tempfile::tempdir().expect("temporary service directory");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private service directory");
    let digest = format!("sha256:{}", "a".repeat(64));
    let identity =
        ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH, &digest).expect("service identity");
    let listener = LocalControlService::bind(&root.path().join("control.sock"), identity)
        .expect("Control listener");
    let manifest = ServiceManifest {
        version: 2,
        service_id: UuidIdentity::try_from(SERVICE_ID.to_owned()).expect("service id"),
        machine_label: MachineLabel::try_from("Sunbook-Pro-M4".to_owned()).expect("machine label"),
        service_epoch: UuidIdentity::try_from(SERVICE_EPOCH.to_owned()).expect("service epoch"),
        control: ControlSelector {
            transport: ControlTransport::UnixJsonLines,
            path: ControlSocketPath::ControlSocket,
        },
        control_schema_digest: SchemaDigest::try_from(digest).expect("schema digest"),
        mcp: McpSelector {
            transport: McpTransport::StreamableHttp,
            url: "http://127.0.0.1:8788/mcp".to_owned(),
        },
        router_proxy_endpoint: None,
    };
    let publication =
        ManifestPublication::publish(root.path(), &manifest).expect("service manifest");
    let stop = CancellationToken::new();
    let server = tokio::spawn(listener.run(stop.clone()));

    let text_output = run_whoami(root.path(), false)
        .await
        .expect("text whoami process");
    assert!(text_output.status.success());
    let text = String::from_utf8(text_output.stdout).expect("UTF-8 text whoami output");
    assert!(text.contains(&format!("machine: {SERVICE_ID} · Sunbook-Pro-M4")));

    let json_output = run_whoami(root.path(), true)
        .await
        .expect("JSON whoami process");
    assert!(json_output.status.success());
    let json: Value = serde_json::from_slice(&json_output.stdout).expect("JSON whoami output");
    assert_eq!(json["result"]["record"]["machineId"], SERVICE_ID);
    assert_eq!(json["result"]["record"]["machineLabel"], "Sunbook-Pro-M4");

    stop.cancel();
    server.await.expect("Control task").expect("Control server");
    drop(publication);
}

async fn run_whoami(
    service_directory: &std::path::Path,
    machine_output: bool,
) -> Result<std::process::Output, std::io::Error> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    command
        .arg("whoami")
        .arg("--service-directory")
        .arg(service_directory)
        .env("CODEX_THREAD_ID", "whoami-machine-proof")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CURSOR_CONVERSATION_ID");
    if machine_output {
        command.arg("--json");
    }
    command.output().await
}
