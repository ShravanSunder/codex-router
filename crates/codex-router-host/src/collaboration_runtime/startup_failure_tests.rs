use super::*;
use crate::{
    ChildCommandSpec, RemoteAccess, tailscale_exposure::PreparedTailscaleExposure,
    tailscale_process::TailscaleCommand,
};
use std::{num::NonZeroU16, os::unix::fs::PermissionsExt as _};

#[tokio::test]
async fn exposure_startup_failure_preserves_the_primary_error_and_closes_constructor_owned_stores()
{
    let root = tempfile::tempdir().expect("private runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private root");
    let script = r#"
import json, sys
if sys.argv[1:] == ['status', '--json']:
    print(json.dumps({'BackendState':'Running','Self':{'DNSName':'node.example.ts.net.'},'CertDomains':['node.example.ts.net']}))
elif sys.argv[1:] == ['serve', 'status', '--json']:
    print('{}')
else:
    sys.exit(23)
"#;
    let command = TailscaleCommand::fixture(
        ChildCommandSpec::new(PathBuf::from("/usr/bin/python3"))
            .with_arguments(["-u", "-c", script]),
    );
    let prepared = PreparedTailscaleExposure::prepare_with_command(
        Some(RemoteAccess::tailscale(
            NonZeroU16::new(8443).expect("port"),
        )),
        command,
    )
    .await
    .expect("preflight");
    let (_, relation) = tokio::sync::watch::channel(RouterExecutableRelation::Match);
    let result = CollaborationRuntime::start_with_optional_router_proxy_endpoint(
        CollaborationRuntimeInputs {
            directory: root.path().to_owned(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("missing-backend.sock"),
            mcp_bind: "127.0.0.1:0".parse().expect("loopback"),
            native_schema: None,
            peer_registry_directory: Some(root.path().join("peers")),
            remote_control_server_name: None,
            owner_human_id: None,
        },
        Vec::new(),
        relation,
        None,
        prepared,
    )
    .await;
    let error = match result {
        Ok(runtime) => {
            runtime
                .shutdown()
                .await
                .expect("unexpected runtime cleanup");
            panic!("Serve child failure must reject startup");
        }
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("Serve"),
        "primary child failure was lost: {message}"
    );
    assert!(
        !message.contains("still owned"),
        "constructor owners outlived startup failure cleanup: {message}"
    );
    assert!(
        !root.path().join("service.json").exists(),
        "failed startup remained published"
    );
    assert!(
        !root.path().join("control.sock").exists(),
        "failed startup retained its API socket"
    );
}
