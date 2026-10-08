use codex_router_host::{CollaborationRuntime, CollaborationRuntimeInputs};
use collaboration_client::CollaborationClient;
use collaboration_protocol::{EndpointAvailability, ObservationTimestamp};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

#[tokio::test]
async fn host_without_provider_faces_does_not_require_owner_lookup()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let runtime = CollaborationRuntime::start(CollaborationRuntimeInputs {
        directory: root.path().to_owned(),
        codex_home: root.path().to_owned(),
        backend_socket: root.path().join("backend.sock"),
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
        peer_registry_directory: None,
        remote_control_server_name: None,
        owner_human_id: None,
    })
    .await?;
    if runtime.owner_human_id().is_some() {
        return Err("Host resolved a provider face owner without provider faces".into());
    }
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn host_serves_the_stateless_collaboration_api_at_the_manifest_url() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    // Arrange
    let root = tempfile::tempdir().expect("temporary runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let runtime = CollaborationRuntime::start(CollaborationRuntimeInputs {
        directory: root.path().to_owned(),
        codex_home: root.path().to_owned(),
        backend_socket: root.path().join("backend.sock"),
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
        peer_registry_directory: None,
        remote_control_server_name: None,
        owner_human_id: None,
    })
    .await
    .expect("runtime starts");
    let manifest: collaboration_protocol::ServiceManifest =
        serde_json::from_slice(&std::fs::read(root.path().join("service.json")).expect("manifest"))
            .expect("manifest decode");
    let authority = manifest
        .mcp
        .url
        .strip_prefix("http://")
        .and_then(|rest| rest.strip_suffix("/mcp"))
        .expect("loopback MCP URL")
        .to_owned();
    let body = serde_json::to_vec(&serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"endpoints_list","arguments":{}}
    }))
    .expect("tool call JSON");

    // Act: one stateless call, with no initialize and no session.
    let mut stream = tokio::net::TcpStream::connect(&authority)
        .await
        .expect("connect to the collaboration API");
    stream
        .write_all(
            format!(
                "POST /mcp HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-11-25\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("request head");
    stream.write_all(&body).await.expect("request body");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("API response");

    // Assert
    let (head, payload) = response.split_once("\r\n\r\n").expect("HTTP response");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let answer: serde_json::Value = serde_json::from_str(payload).expect("JSON-RPC response");
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    assert_eq!(
        answer["result"]["structuredContent"]["serviceEpoch"],
        serde_json::json!(runtime.service_epoch())
    );
    runtime.shutdown().await.expect("runtime shutdown");
}

#[tokio::test]
async fn post_bind_manifest_failure_releases_mcp_port() {
    let root = tempfile::tempdir().expect("temporary runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    std::fs::create_dir(root.path().join("service.json")).expect("blocking manifest destination");
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").expect("port reservation");
    let mcp_bind = reservation.local_addr().expect("reserved address");
    drop(reservation);

    let result = CollaborationRuntime::start(CollaborationRuntimeInputs {
        directory: root.path().to_owned(),
        codex_home: root.path().to_owned(),
        backend_socket: root.path().join("backend.sock"),
        mcp_bind,
        native_schema: None,
        peer_registry_directory: None,
        remote_control_server_name: None,
        owner_human_id: None,
    })
    .await;
    assert!(result.is_err());
    tokio::task::yield_now().await;
    let rebound = tokio::net::TcpListener::bind(mcp_bind)
        .await
        .expect("MCP port released after later startup failure");
    drop(rebound);
}

#[tokio::test]
async fn host_composes_discovery_and_retires_only_owned_communication_sockets() {
    let root = std::path::PathBuf::from(format!("/tmp/host-collaboration-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|e| panic!("directory: {e}"));
    let inputs = || CollaborationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("backend.sock"),
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
        peer_registry_directory: None,
        remote_control_server_name: Some(
            "remote-control-fixture"
                .to_owned()
                .try_into()
                .expect("valid fixture Remote Control name"),
        ),
        owner_human_id: Some(
            "test-owner"
                .to_owned()
                .try_into()
                .expect("test owner identity"),
        ),
    };
    let mut runtime = CollaborationRuntime::start(inputs())
        .await
        .unwrap_or_else(|e| panic!("start: {e}"));
    assert_eq!(
        runtime.owner_human_id().map(message_board::HumanId::as_str),
        Some("test-owner")
    );
    assert!(root.join("provider-operations.sqlite").is_file());
    let manifest: collaboration_protocol::ServiceManifest = serde_json::from_slice(
        &std::fs::read(root.join("service.json")).unwrap_or_else(|e| panic!("manifest read: {e}")),
    )
    .unwrap_or_else(|e| panic!("manifest decode: {e}"));
    assert_eq!(&manifest.service_id, runtime.service_id());
    assert_eq!(manifest.machine_label.as_str(), "remote-control-fixture");
    assert_eq!(manifest.version, 3);
    assert_eq!(
        manifest.api.path,
        collaboration_protocol::ApiSocketPath::ServiceSocket
    );
    assert_eq!(manifest.native_schema_digest, None);
    assert_eq!(
        manifest.mcp.transport,
        collaboration_protocol::McpTransport::StreamableHttp
    );
    assert!(manifest.mcp.url.starts_with("http://127.0.0.1:"));
    assert!(manifest.mcp.url.ends_with("/mcp"));
    let control_schema_files = std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("service directory: {error}"))
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("control-schema-")
        })
        .count();
    assert_eq!(control_schema_files, 0, "no Control schema is published");
    let original_id = runtime.service_id().clone();
    let original_epoch = runtime.service_epoch().clone();
    let client = CollaborationClient::connect(&root, "host-proof", "1")
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
            collaboration_protocol::ChannelDescription::NativeCodex { schema_digest, .. } => {
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
    let collaboration_client::JournalStatus::Available { bounds } = status else {
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
            collaboration_protocol::JournalPosition {
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
            if addresses.coverage.state == collaboration_protocol::CoverageState::Observing {
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
        collaboration_protocol::LifecycleChange::BackendStatus {
            status: collaboration_protocol::BackendStatus::Ready
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
        collaboration_protocol::CoverageState::Disconnected
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
    let second = CollaborationRuntime::start(inputs())
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
    std::fs::remove_file(schema_path).unwrap_or_else(|error| panic!("schema cleanup: {error}"));
    std::fs::remove_file(root.join("backend.sock"))
        .unwrap_or_else(|error| panic!("backend cleanup: {error}"));
    std::fs::remove_file(executable).unwrap_or_else(|error| panic!("generator cleanup: {error}"));
    std::fs::remove_file(export_directory.join("codex_app_server_protocol.schemas.json"))
        .unwrap_or_else(|error| panic!("export cleanup: {error}"));
    std::fs::remove_dir(export_directory)
        .unwrap_or_else(|error| panic!("export directory cleanup: {error}"));
    std::fs::remove_file(root.join("project-board.sqlite"))
        .unwrap_or_else(|error| panic!("board database cleanup: {error}"));
    std::fs::remove_file(root.join("automation.sqlite"))
        .unwrap_or_else(|error| panic!("automation database cleanup: {error}"));
    std::fs::remove_file(root.join("provider-operations.sqlite"))
        .unwrap_or_else(|error| panic!("provider operation database cleanup: {error}"));
    std::fs::remove_dir(root).unwrap_or_else(|e| panic!("directory cleanup: {e}"));
}

#[tokio::test]
async fn occupied_mcp_port_is_a_visible_startup_failure_without_fallback() {
    let root = tempfile::tempdir().expect("temporary runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("occupied listener");
    let mcp_bind = occupied.local_addr().expect("occupied address");
    let result = CollaborationRuntime::start(CollaborationRuntimeInputs {
        directory: root.path().to_owned(),
        codex_home: root.path().to_owned(),
        backend_socket: root.path().join("backend.sock"),
        mcp_bind,
        native_schema: None,
        peer_registry_directory: None,
        remote_control_server_name: None,
        owner_human_id: None,
    })
    .await;
    let error = match result {
        Ok(_) => panic!("occupied MCP port must fail startup"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
    drop(occupied);
}
