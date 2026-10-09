use collaboration_mcp::test_support::ServedCollaborationApi;
use collaboration_protocol::UuidIdentity;
use collaboration_service::{CollaborationApplication, MachineIdentity, ServiceIdentity};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;

const SERVICE_ID: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";
const SERVICE_EPOCH: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";

#[tokio::test]
async fn whoami_displays_service_id_and_machine_label_in_text_and_json() {
    let root = tempfile::tempdir().expect("temporary service directory");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private service directory");
    // The CLI reads the machine label from the version 3 manifest the API publishes.
    let machine_identity = MachineIdentity::new(
        UuidIdentity::try_from(SERVICE_ID.to_owned()).expect("service id"),
        Some("Sunbook-Pro-M4"),
    )
    .expect("machine label");
    let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH)
        .expect("service identity")
        .with_machine_identity(machine_identity)
        .expect("machine identity");
    let served =
        ServedCollaborationApi::start(root.path(), CollaborationApplication::new(identity))
            .await
            .expect("served collaboration API");

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

    served.stop().await.expect("collaboration API stops");
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
