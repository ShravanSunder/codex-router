use super::{HostLaunchMode, HostReplacementCommandInputs, host_replacement_command};
use codex_router_host::{ChildCommandSpec, RemoteAccess};
use std::{ffi::OsString, num::NonZeroU16, path::PathBuf};

#[test]
fn replacement_command_preserves_private_tailnet_mode_and_selected_port() {
    let executable = PathBuf::from("/tmp/debug Router/codex-router");
    let command = host_replacement_command(HostReplacementCommandInputs {
        executable: executable.clone(),
        router_root: PathBuf::from("/tmp/private Router"),
        port: 18787,
        mcp_bind: "127.0.0.1:18788".parse().expect("loopback"),
        remote_access: Some(RemoteAccess::tailscale(
            NonZeroU16::new(9443).expect("port"),
        )),
        provider_operation_retention_days: std::num::NonZeroU32::new(60).expect("days"),
        owner_human_id: None,
        external_provider_launches: &[],
        launch_mode: HostLaunchMode::IsolatedDebug,
    });

    assert_eq!(
        command,
        ChildCommandSpec::new(executable).with_arguments(
            [
                "host",
                "--router-root",
                "/tmp/private Router",
                "--port",
                "18787",
                "--mcp-bind",
                "127.0.0.1:18788",
                "--provider-operation-retention-days",
                "60",
                "--remote-access",
                "tailscale",
                "--tailscale-https-port",
                "9443",
                "--require-debug-isolation",
            ]
            .map(OsString::from)
        )
    );
}
