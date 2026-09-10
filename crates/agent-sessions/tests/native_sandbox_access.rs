//! Opt-in OS sandbox proof with a real Control socket; no app-server or model is launched.
use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_protocol::OperationId;
use std::{
    ffi::OsString,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Output,
};
type ProofResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore = "requires installed Codex, existing debug profile and macOS Seatbelt permissions"]
async fn codex_sandbox_requires_exact_control_socket_permission() -> ProofResult<()> {
    if !cfg!(target_os = "macos") {
        return Err("This permission proof requires macOS".into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME unavailable")?).join(".codex");
    codex_native_integration::DebugCodexProfile::read(&home, 18787)?;
    let executable =
        codex_native_integration::CodexPaths::from_codex_home(home.clone()).managed_executable();
    let root =
        PathBuf::from("/tmp").join(format!("ipc-proof-{}", OperationId::generate().as_str()));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let runtime = CommunicationRuntime::start(CommunicationRuntimeInputs {
        directory: root.clone(),
        codex_home: home,
        backend_socket: root.join("absent.sock"),
        native_schema: None,
    })
    .await?;
    let command: Vec<OsString> = vec![
        env!("CARGO_BIN_EXE_agent-sessions").into(),
        "endpoints".into(),
        "list".into(),
        "--service-directory".into(),
        root.clone().into_os_string(),
        "--json".into(),
    ];
    let sandbox = SandboxProbe {
        executable: &executable,
        root: &root,
    };
    let denied = sandbox.run(SocketGrant::None, &command).await?;
    if denied.status.success() {
        return Err(
            "Default network-disabled workspace unexpectedly admitted Control socket access".into(),
        );
    }
    let allowed = sandbox
        .run(
            SocketGrant::CommandFlag(&root.join("control.sock")),
            &command,
        )
        .await?;
    if !allowed.status.success() {
        return Err(format!(
            "Exact socket permission failed: {} {}",
            String::from_utf8_lossy(&allowed.stdout),
            String::from_utf8_lossy(&allowed.stderr)
        )
        .into());
    }
    let response: serde_json::Value = serde_json::from_slice(&allowed.stdout)?;
    if response
        .pointer("/result/endpoints")
        .and_then(serde_json::Value::as_array)
        .is_none()
    {
        return Err("Allowed CLI did not return real endpoint discovery".into());
    }
    let forbidden = root.join("other-control.sock");
    let listener = tokio::net::UnixListener::bind(&forbidden)?;
    let probe = vec![
        OsString::from("/usr/bin/python3"),
        OsString::from("-c"),
        OsString::from(
            "import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])",
        ),
        forbidden.clone().into_os_string(),
    ];
    let denied_other = sandbox
        .run(SocketGrant::CommandFlag(&root.join("control.sock")), &probe)
        .await?;
    if denied_other.status.success()
        || !String::from_utf8_lossy(&denied_other.stderr).contains("PermissionError")
    {
        return Err(
            "Exact socket grant did not retain denial for a different listening socket".into(),
        );
    }
    let configured = sandbox
        .run(
            SocketGrant::Configuration(&root.join("control.sock")),
            &command,
        )
        .await?;
    if !configured.status.success() {
        return Err(format!(
            "Native configuration socket grant failed: {} {}",
            String::from_utf8_lossy(&configured.stdout),
            String::from_utf8_lossy(&configured.stderr)
        )
        .into());
    }
    let configured_denial = sandbox
        .run(
            SocketGrant::Configuration(&root.join("control.sock")),
            &probe,
        )
        .await?;
    if configured_denial.status.success()
        || !String::from_utf8_lossy(&configured_denial.stderr).contains("PermissionError")
    {
        return Err("Native configuration widened access to another listening socket".into());
    }
    println!("Native managed-network configuration also permits only the selected Control socket.");
    println!(
        "Default workspace socket denied; exact Control socket grant passed; other listening socket remained denied."
    );
    drop(listener);
    runtime.shutdown().await?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            return Err("unexpected fixture subdirectory".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
struct SandboxProbe<'a> {
    executable: &'a Path,
    root: &'a Path,
}
enum SocketGrant<'a> {
    None,
    CommandFlag(&'a Path),
    Configuration(&'a Path),
}
impl SandboxProbe<'_> {
    async fn run(&self, grant: SocketGrant<'_>, command: &[OsString]) -> ProofResult<Output> {
        let mut child = tokio::process::Command::new(self.executable);
        child
            .args([
                "sandbox",
                "--profile",
                "codex-router-debug",
                "--permission-profile",
                if matches!(grant, SocketGrant::Configuration(_)) {
                    "ipc-proof"
                } else {
                    ":workspace"
                },
                "--cd",
            ])
            .arg(self.root)
            .args(["--disable", "hooks"]);
        match grant {
            SocketGrant::Configuration(socket) => {
                let reservation = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
                let port = reservation.local_addr()?.port();
                let path = socket.canonicalize()?;
                let proxy = format!(
                    "features.network_proxy={{ enabled=true, proxy_url=\"http://127.0.0.1:{port}\", enable_socks5=false, allow_upstream_proxy=false, allow_local_binding=false, credential_broker=false, domains={{}}, unix_sockets={{ \"{}\"=\"allow\" }} }}",
                    path.display()
                );
                child.args([
                    "-c",
                    "permissions.ipc-proof={extends=\":workspace\",network={enabled=true}}",
                    "-c",
                    &proxy,
                ]);
                drop(reservation);
            }
            SocketGrant::None | SocketGrant::CommandFlag(_) => {
                child.args([
                    "-c",
                    "sandbox_workspace_write.network_access=false",
                    "--disable",
                    "network_proxy",
                ]);
                if let SocketGrant::CommandFlag(socket) = grant {
                    child.arg("--allow-unix-socket").arg(socket);
                }
            }
        }
        child.arg("--").args(command).kill_on_drop(true);
        Ok(tokio::time::timeout(std::time::Duration::from_secs(30), child.output()).await??)
    }
}
