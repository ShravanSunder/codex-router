use crate::{CollaborationRuntime, CollaborationRuntimeInputs, HostCollaborationInputs};
use collaboration_protocol::{RouterExecutableRelation, ServiceManifest};
use std::{net::SocketAddr, os::unix::fs::PermissionsExt as _};

fn runtime_inputs(directory: &std::path::Path) -> CollaborationRuntimeInputs {
    CollaborationRuntimeInputs {
        directory: directory.to_owned(),
        codex_home: directory.to_owned(),
        backend_socket: directory.join("backend.sock"),
        mcp_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
        peer_registry_directory: Some(directory.join("claude-sessions")),
        remote_control_server_name: None,
        owner_human_id: None,
    }
}

fn service_manifest(directory: &std::path::Path) -> ServiceManifest {
    serde_json::from_slice(
        &std::fs::read(directory.join("service.json")).expect("published service manifest"),
    )
    .expect("typed service manifest")
}

#[tokio::test]
async fn host_startup_publishes_the_configured_router_proxy_port() {
    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let router_proxy_endpoint: SocketAddr = "127.0.0.1:18787".parse().expect("custom Router port");
    let (_relation_sender, relation_receiver) =
        tokio::sync::watch::channel(RouterExecutableRelation::Match);

    let runtime = CollaborationRuntime::start_for_host_with_router_proxy_endpoint(
        HostCollaborationInputs {
            collaboration_runtime: runtime_inputs(root.path()),
            router_proxy_endpoint,
        },
        Vec::new(),
        relation_receiver,
    )
    .await
    .expect("Host collaboration runtime");
    let manifest = service_manifest(root.path());
    runtime.shutdown().await.expect("runtime shutdown");

    assert_eq!(manifest.router_proxy_endpoint, Some(router_proxy_endpoint));
}

#[tokio::test]
async fn direct_runtime_publication_omits_the_host_router_proxy_endpoint() {
    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");

    let runtime = CollaborationRuntime::start(runtime_inputs(root.path()))
        .await
        .expect("direct collaboration runtime");
    let manifest = service_manifest(root.path());
    runtime.shutdown().await.expect("runtime shutdown");

    assert_eq!(manifest.router_proxy_endpoint, None);
}

/// A Codex native schema export from a generator fixture, as the Host receives at startup.
async fn native_schema_export(
    directory: &std::path::Path,
) -> codex_native_integration::NativeSchemaExport {
    let executable = directory.join("schema-generator");
    let mut definitions = serde_json::Map::new();
    for name in ["ThreadRead", "ThreadStart", "TurnStart"] {
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
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\ncat > \"$5/codex_app_server_protocol.schemas.json\" <<'SCHEMA'\n{schema}\nSCHEMA\n"
        ),
    )
    .expect("schema generator fixture");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .expect("schema generator permissions");
    let identity = codex_native_integration::executable_identity(&executable)
        .await
        .expect("schema generator identity");
    codex_native_integration::NativeSchemaExport::generate(
        &identity,
        &directory.join("schema-export"),
    )
    .await
    .expect("native schema export")
}

#[tokio::test]
async fn host_publishes_a_version_three_manifest_naming_the_api_socket() {
    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let export = native_schema_export(root.path()).await;
    let native_digest = format!(
        "sha256:{}",
        export
            .bundle()
            .digest()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let mut inputs = runtime_inputs(root.path());
    inputs.native_schema = Some(std::sync::Arc::new(export));

    let runtime = CollaborationRuntime::start(inputs)
        .await
        .expect("collaboration runtime");
    let published: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.path().join("service.json")).expect("published service manifest"),
    )
    .expect("manifest JSON");
    let manifest = service_manifest(root.path());
    let socket_mode = std::fs::metadata(root.path().join("control.sock"))
        .expect("API socket")
        .permissions()
        .mode()
        & 0o777;
    let client =
        collaboration_client::CollaborationClient::connect(root.path(), "manifest-proof", "1")
            .await;
    runtime.shutdown().await.expect("runtime shutdown");

    assert_eq!(published["version"], 3);
    assert_eq!(
        published["api"],
        serde_json::json!({"transport":"streamableHttpUnix","path":"control.sock"})
    );
    assert_eq!(published["serviceVersion"], env!("CARGO_PKG_VERSION"));
    assert!(
        published["mcp"]["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("http://127.0.0.1:") && url.ends_with("/mcp")),
        "{published}"
    );
    assert_eq!(published["nativeSchemaDigest"], native_digest.as_str());
    assert!(published.get("control").is_none(), "{published}");
    assert!(
        published.get("controlSchemaDigest").is_none(),
        "{published}"
    );
    assert_eq!(socket_mode, 0o600, "the API socket is owner-only");
    let client = client.expect("the CLIs' client reads the version 3 manifest");
    assert_eq!(client.identity().service_id, manifest.service_id);
    assert_eq!(client.identity().service_epoch, manifest.service_epoch);
}

#[tokio::test]
async fn the_cli_client_refuses_a_version_two_manifest_from_the_host_directory() {
    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let runtime = CollaborationRuntime::start(runtime_inputs(root.path()))
        .await
        .expect("collaboration runtime");
    let manifest_path = root.path().join("service.json");
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).expect("published manifest"))
            .expect("manifest JSON");
    let fields = legacy.as_object_mut().expect("manifest object");
    for field in ["api", "serviceVersion", "nativeSchemaDigest"] {
        fields.remove(field);
    }
    fields.insert("version".to_owned(), serde_json::json!(2));
    fields.insert(
        "control".to_owned(),
        serde_json::json!({"transport":"unixJsonLines","path":"control.sock"}),
    );
    fields.insert(
        "controlSchemaDigest".to_owned(),
        serde_json::json!(format!("sha256:{}", "a".repeat(64))),
    );
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&legacy).expect("legacy manifest"),
    )
    .expect("rewrite the manifest as version 2");

    let refused =
        collaboration_client::CollaborationClient::connect(root.path(), "legacy-proof", "1").await;
    runtime.shutdown().await.expect("runtime shutdown");

    let error = refused
        .err()
        .expect("a version 2 manifest is refused")
        .to_string();
    assert!(
        error.contains("unsupported service manifest version"),
        "{error}"
    );
}
