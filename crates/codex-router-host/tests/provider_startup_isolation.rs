#![allow(clippy::expect_used, clippy::indexing_slicing)]
//! Fail-fast fixture assertions at the Host endpoint boundary.

use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    ExternalProviderStartup,
};
use collaboration_client::ControlClient;
use collaboration_protocol::{EndpointAvailability, EndpointDescription, ObservationTimestamp};
use std::{os::unix::fs::PermissionsExt as _, path::Path};

fn runtime_inputs(directory: &Path) -> CollaborationRuntimeInputs {
    CollaborationRuntimeInputs {
        directory: directory.to_owned(),
        codex_home: directory.to_owned(),
        backend_socket: directory.join("backend.sock"),
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
        peer_registry_directory: None,
    }
}

fn fixture_provider(directory: &Path) -> std::path::PathBuf {
    let executable = directory.join("fixture-provider.py");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/python3
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'agentInfo':{'name':'startup-fixture','version':'1'}}})); sys.stdout.flush()
sys.stdin.read()
"#,
    )
    .expect("write fixture provider");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .expect("make fixture executable");
    executable
}

fn endpoint<'a>(endpoints: &'a [EndpointDescription], id: &str) -> &'a EndpointDescription {
    endpoints
        .iter()
        .find(|endpoint| String::from(endpoint.endpoint.endpoint_id.clone()) == id)
        .expect("named endpoint")
}

#[tokio::test]
async fn missing_executable_isolated_from_other_provider_and_codex() {
    let root = tempfile::tempdir().expect("router root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private root");
    let cursor = fixture_provider(root.path());

    let mut runtime = CollaborationRuntime::start_with_external_providers(
        runtime_inputs(root.path()),
        vec![
            ExternalProviderStartup::Launch(
                ExternalProviderLaunchBinding::claude(
                    root.path().join("missing-claude-agent-acp"),
                    Vec::new(),
                )
                .expect("Claude binding"),
            ),
            ExternalProviderStartup::Launch(
                ExternalProviderLaunchBinding::cursor(cursor, Vec::new()).expect("Cursor binding"),
            ),
        ],
    )
    .await
    .expect("Host survives one unavailable provider");
    runtime
        .backend_ready(
            ObservationTimestamp::try_from("2026-09-24T12:00:00Z".to_owned()).expect("timestamp"),
            None,
        )
        .await
        .expect("Codex endpoint ready");
    let mut client = ControlClient::connect(root.path(), "startup-isolation", "1")
        .await
        .expect("Control client");
    let inventory = client.list_endpoints().await.expect("endpoint inventory");

    assert!(
        !endpoint(&inventory.endpoints, "codex-local")
            .channels
            .is_empty()
    );
    assert!(matches!(
        endpoint(&inventory.endpoints, "cursor-local").availability,
        EndpointAvailability::Available { .. }
    ));
    let claude = endpoint(&inventory.endpoints, "claude-local");
    assert!(claude.channels.is_empty());
    let EndpointAvailability::Unavailable { reason, fix, .. } = &claude.availability else {
        panic!("missing executable must be unavailable")
    };
    assert!(String::from(reason.clone()).contains("missing-claude-agent-acp"));
    assert!(String::from(fix.clone().expect("fix")).contains("--claude-acp-executable"));
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn failed_initialize_isolated_from_other_provider() {
    let root = tempfile::tempdir().expect("router root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private root");
    let cursor = fixture_provider(root.path());
    let failing = root.path().join("failing-provider.py");
    std::fs::write(&failing, "#!/usr/bin/python3\nimport sys\nsys.exit(4)\n")
        .expect("write failure fixture");
    std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o700))
        .expect("make failure fixture executable");

    let runtime = CollaborationRuntime::start_with_external_providers(
        runtime_inputs(root.path()),
        vec![
            ExternalProviderStartup::Launch(
                ExternalProviderLaunchBinding::claude(failing, Vec::new()).expect("Claude binding"),
            ),
            ExternalProviderStartup::Launch(
                ExternalProviderLaunchBinding::cursor(cursor, Vec::new()).expect("Cursor binding"),
            ),
        ],
    )
    .await
    .expect("Host survives initialization failure");
    let mut client = ControlClient::connect(root.path(), "initialize-isolation", "1")
        .await
        .expect("Control client");
    let inventory = client.list_endpoints().await.expect("endpoint inventory");

    assert!(matches!(
        endpoint(&inventory.endpoints, "claude-local").availability,
        EndpointAvailability::Unavailable { .. }
    ));
    assert!(matches!(
        endpoint(&inventory.endpoints, "cursor-local").availability,
        EndpointAvailability::Available { .. }
    ));
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn disabled_provider_is_listed_without_a_transport() {
    let root = tempfile::tempdir().expect("router root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private root");
    let cursor = fixture_provider(root.path());
    let disabled = ExternalProviderStartup::unavailable(
        collaboration_protocol::ProviderKind::ClaudeCode,
        "disabled in providers.json".to_owned(),
        "enable Claude in providers.json and restart the Host".to_owned(),
    )
    .expect("disabled endpoint");

    let runtime = CollaborationRuntime::start_with_external_providers(
        runtime_inputs(root.path()),
        vec![
            disabled,
            ExternalProviderStartup::Launch(
                ExternalProviderLaunchBinding::cursor(cursor, Vec::new()).expect("Cursor binding"),
            ),
        ],
    )
    .await
    .expect("Host with disabled Claude");
    let mut client = ControlClient::connect(root.path(), "disabled-provider", "1")
        .await
        .expect("Control client");
    let inventory = client.list_endpoints().await.expect("endpoint inventory");

    let claude = endpoint(&inventory.endpoints, "claude-local");
    assert!(claude.channels.is_empty());
    let EndpointAvailability::Unavailable { reason, fix, .. } = &claude.availability else {
        panic!("disabled provider must be unavailable")
    };
    assert_eq!(String::from(reason.clone()), "disabled in providers.json");
    assert!(fix.is_some());
    assert!(matches!(
        endpoint(&inventory.endpoints, "cursor-local").availability,
        EndpointAvailability::Available { .. }
    ));
    runtime.shutdown().await.expect("shutdown");
}
