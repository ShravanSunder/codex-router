use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_client::ControlClient;
use communication_protocol::{EndpointAvailability, ObservationTimestamp};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

#[tokio::test]
async fn host_composes_discovery_and_retires_only_owned_communication_sockets() {
    let root = std::path::PathBuf::from(format!("/tmp/host-communication-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|e| panic!("directory: {e}"));
    let inputs = || CommunicationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("backend.sock"),
        native_schema: None,
    };
    let mut runtime = CommunicationRuntime::start(inputs())
        .await
        .unwrap_or_else(|e| panic!("start: {e}"));
    let manifest: communication_protocol::ServiceManifest = serde_json::from_slice(
        &std::fs::read(root.join("service.json")).unwrap_or_else(|e| panic!("manifest read: {e}")),
    )
    .unwrap_or_else(|e| panic!("manifest decode: {e}"));
    assert_eq!(&manifest.service_id, runtime.service_id());
    let control_digest = String::from(manifest.control_schema_digest.clone());
    let control_schema_path = root.join(format!(
        "control-schema-{}.json",
        control_digest.trim_start_matches("sha256:")
    ));
    let control_schema = communication_protocol::ControlSchema::generate(None)
        .unwrap_or_else(|error| panic!("control schema: {error}"));
    assert_eq!(
        std::fs::read(&control_schema_path)
            .unwrap_or_else(|error| panic!("published Control schema: {error}")),
        control_schema.bytes()
    );
    let original_id = runtime.service_id().clone();
    let original_epoch = runtime.service_epoch().clone();
    let mut client = ControlClient::connect(&root, "host-proof", "1")
        .await
        .unwrap_or_else(|e| panic!("discovery: {e}"));
    let executable = root.join("schema-generator");
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ] {
        definitions.insert(
            format!("{name}Params"),
            serde_json::json!({"type":"object"}),
        );
        definitions.insert(
            format!("{name}Response"),
            serde_json::json!({"type":"object"}),
        );
    }
    let schema = serde_json::json!({"definitions":{"v2":definitions}});
    std::fs::write(&executable, format!("#!/bin/sh\ncat > \"$5/codex_app_server_protocol.schemas.json\" <<'SCHEMA'\n{schema}\nSCHEMA\n"))
        .unwrap_or_else(|error| panic!("generator: {error}"));
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("permissions: {error}"));
    let executable_identity = codex_native_integration::executable_identity(&executable)
        .await
        .unwrap_or_else(|error| panic!("identity: {error}"));
    let export_directory = root.join("schema-export");
    let export = codex_native_integration::NativeSchemaExport::generate(
        &executable_identity,
        &export_directory,
    )
    .await
    .unwrap_or_else(|error| panic!("export: {error}"));
    let backend = tokio::net::UnixListener::bind(root.join("backend.sock"))
        .unwrap_or_else(|error| panic!("backend fixture: {error}"));
    let backend_task = tokio::spawn(async move {
        use futures_util::{SinkExt, StreamExt};
        let (stream, _) = backend
            .accept()
            .await
            .unwrap_or_else(|error| panic!("accept: {error}"));
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .unwrap_or_else(|error| panic!("upgrade: {error}"));
        for method in ["initialize", "initialized", "thread/loaded/list"] {
            let frame = socket
                .next()
                .await
                .unwrap_or_else(|| panic!("frame"))
                .unwrap_or_else(|error| panic!("read: {error}"));
            let request: serde_json::Value = serde_json::from_str(
                frame
                    .to_text()
                    .unwrap_or_else(|error| panic!("text: {error}")),
            )
            .unwrap_or_else(|error| panic!("JSON: {error}"));
            assert_eq!(request["method"], method);
            if method != "initialized" {
                let result = if method == "initialize" {
                    serde_json::json!({})
                } else {
                    serde_json::json!({"data":[],"nextCursor":null})
                };
                socket
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        serde_json::json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap_or_else(|error| panic!("send: {error}"));
            }
        }
        let _retired = socket.next().await;
    });
    runtime
        .backend_ready(
            ObservationTimestamp::try_from("2026-09-05T12:00:00Z".to_owned())
                .unwrap_or_else(|e| panic!("time: {e}")),
            Some(codex_router_host::BackendSchemaEvidence {
                running_executable: &executable_identity,
                export: &export,
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("ready: {e}"));
    let endpoints = client
        .list_endpoints()
        .await
        .unwrap_or_else(|e| panic!("list: {e}"));
    assert!(matches!(
        endpoints.endpoints.first().map(|entry| &entry.availability),
        Some(EndpointAvailability::Available { .. })
    ));
    let schema_digest = endpoints
        .endpoints
        .first()
        .and_then(|endpoint| endpoint.channels.first())
        .and_then(|channel| match channel {
            communication_protocol::ChannelDescription::NativeCodex { schema_digest, .. } => {
                schema_digest.clone()
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("schema digest missing"));
    let encoded: String = schema_digest.into();
    let schema_path = root.join(format!(
        "{}.json",
        encoded
            .strip_prefix("sha256:")
            .unwrap_or_else(|| panic!("digest prefix"))
    ));
    assert_eq!(
        std::fs::read(&schema_path).unwrap_or_else(|error| panic!("published schema: {error}")),
        export.bundle().canonical_bytes()
    );
    let status = client
        .journal_status()
        .await
        .unwrap_or_else(|e| panic!("journal status: {e}"));
    let communication_client::JournalStatus::Available { bounds } = status else {
        panic!("journal unavailable");
    };
    let target = endpoints
        .endpoints
        .first()
        .unwrap_or_else(|| panic!("endpoint"))
        .endpoint
        .clone();
    let page = client
        .read_journal(
            &target,
            communication_protocol::JournalPosition {
                journal_id: bounds.journal_id,
                sequence: 0,
            },
            100,
            0,
        )
        .await
        .unwrap_or_else(|e| panic!("journal read: {e}"));
    let addresses = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let addresses = client
                .list_addresses(&target, 100, None)
                .await
                .unwrap_or_else(|error| panic!("address snapshot: {error}"));
            if addresses.coverage.state == communication_protocol::CoverageState::Observing {
                break addresses;
            }
            client
                .read_journal(&target, addresses.watermark, 100, 1000)
                .await
                .unwrap_or_else(|error| panic!("wait for coverage: {error}"));
        }
    })
    .await
    .unwrap_or_else(|error| panic!("coverage deadline: {error}"));
    assert!(addresses.entries.is_empty());
    assert_eq!(addresses.coverage.endpoint, target);
    assert!(addresses.watermark.sequence >= page.next.sequence);
    assert!(page.records.iter().any(|record| matches!(
        record.observation.change,
        communication_protocol::LifecycleChange::BackendStatus {
            status: communication_protocol::BackendStatus::Ready
        }
    )));
    runtime
        .backend_unavailable(
            "2026-09-05T12:01:00Z"
                .to_owned()
                .try_into()
                .unwrap_or_else(|error| panic!("time: {error}")),
            "Fixture retired"
                .to_owned()
                .try_into()
                .unwrap_or_else(|error| panic!("reason: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("retire: {error}"));
    let retired = client
        .list_addresses(&target, 100, None)
        .await
        .unwrap_or_else(|error| panic!("retired snapshot: {error}"));
    assert_eq!(
        retired.coverage.state,
        communication_protocol::CoverageState::Disconnected
    );
    backend_task
        .await
        .unwrap_or_else(|error| panic!("backend fixture: {error}"));
    runtime
        .shutdown()
        .await
        .unwrap_or_else(|e| panic!("shutdown: {e}"));
    assert!(!root.join("service.json").exists());
    assert!(!root.join("control.sock").exists());
    assert!(!root.join("codex-native.sock").exists());
    let second = CommunicationRuntime::start(inputs())
        .await
        .unwrap_or_else(|e| panic!("restart: {e}"));
    assert_eq!(second.service_id(), &original_id);
    assert_ne!(second.service_epoch(), &original_epoch);
    second
        .shutdown()
        .await
        .unwrap_or_else(|e| panic!("shutdown: {e}"));
    std::fs::remove_file(root.join("session-registry.sqlite"))
        .unwrap_or_else(|e| panic!("database cleanup: {e}"));
    std::fs::remove_file(root.join("service-identity.json"))
        .unwrap_or_else(|e| panic!("identity cleanup: {e}"));
    std::fs::remove_file(control_schema_path)
        .unwrap_or_else(|error| panic!("Control schema cleanup: {error}"));
    std::fs::remove_file(schema_path).unwrap_or_else(|error| panic!("schema cleanup: {error}"));
    std::fs::remove_file(root.join("backend.sock"))
        .unwrap_or_else(|error| panic!("backend cleanup: {error}"));
    std::fs::remove_file(executable).unwrap_or_else(|error| panic!("generator cleanup: {error}"));
    std::fs::remove_file(export_directory.join("codex_app_server_protocol.schemas.json"))
        .unwrap_or_else(|error| panic!("export cleanup: {error}"));
    std::fs::remove_dir(export_directory)
        .unwrap_or_else(|error| panic!("export directory cleanup: {error}"));
    std::fs::remove_file(root.join("automation.sqlite"))
        .unwrap_or_else(|error| panic!("automation database cleanup: {error}"));
    std::fs::remove_dir(root).unwrap_or_else(|e| panic!("directory cleanup: {e}"));
}
