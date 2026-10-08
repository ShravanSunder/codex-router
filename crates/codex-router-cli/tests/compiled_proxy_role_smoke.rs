#![cfg(feature = "keychain-test-support")]
use std::{path::Path, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
#[tokio::test]
async fn compiled_fresh_serve_uses_role_activation_and_actual_hyper()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let gate = codex_router_descriptor_boundary::DescriptorGate::global();
    let port_reservation =
        codex_router_descriptor_boundary::OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate)
            .await?;
    let reserved_port = port_reservation.tcp_address()?.port().to_string();
    drop(port_reservation);
    let mut command =
        tokio::process::Command::new(repository.join("scripts/cargo_debug_signing_runner.sh"));
    command
        .arg(env!("CARGO_BIN_EXE_codex-router"))
        .args([
            "serve",
            "--listen-host",
            "127.0.0.1",
            "--port",
            &reserved_port,
            "--max-connections",
            "1",
            "--disable-background-quota-refresh",
            "--upstream-base-url",
            "http://127.0.0.1:1/v1",
            "--require-debug-isolation",
        ])
        .arg("--state-db")
        .arg(root.path().join("state.sqlite"))
        .arg("--secret-root")
        .arg(root.path().join("secrets"))
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env_remove("CODEX_ROUTER_OBSERVABILITY_MARKER")
        .current_dir(&repository)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = codex_router_descriptor_boundary::DescriptorGate::global()
        .spawn_child(&mut command)
        .await?;
    let pid = child.id().ok_or("compiled fixture has a real PID")?;
    let stdout = child.stdout.take().ok_or("compiled fixture stdout")?;
    let stderr = child.stderr.take().ok_or("compiled fixture stderr")?;
    let stderr_task = tokio::spawn(async move {
        let mut stream = stderr;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let result=async {
        let mut lines=BufReader::new(stdout).lines();
        let address=tokio::time::timeout(Duration::from_secs(30),async {
            while let Some(line)=lines.next_line().await? {
                if let Some(address)=line.strip_prefix("listening: "){return address.parse::<std::net::SocketAddr>().map_err(std::io::Error::other);}
            }
            Err(std::io::Error::other("compiled CLI exited before actual listening output"))
        }).await??;
        if !address.ip().is_loopback() { return Err("compiled listener must be loopback".into()); }
        if address.port() == 0 { return Err("compiled listener must have a nonzero port".into()); }
        let mut client=tokio::net::TcpStream::connect(address).await?;
        client.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await?;
        let mut reply=Vec::new();tokio::time::timeout(Duration::from_secs(2),client.read_to_end(&mut reply)).await??;
        if !reply.starts_with(b"HTTP/1.1 200") { return Err("actual compiled Hyper health response must be HTTP 200".into()); }
        let status=tokio::time::timeout(Duration::from_secs(5),child.wait()).await??;
        if !status.success() { return Err(format!("actual compiled child exit must be successful: {status}").into()); }
        eprintln!("compiled Fresh Serve fixture pid={pid} address={address} normal_exit={} literal_http_200=true",status.code().ok_or("normal exit code")?);
        if !root.path().join("state.sqlite").is_file() { return Err("actual Fresh activation must create the state database".into()); }
        Ok::<(),Box<dyn std::error::Error>>(())
    }.await;
    if result.is_err() {
        let _kill = child.start_kill();
        let _reaped = child.wait().await?;
    }
    let stderr = stderr_task.await??;
    for line in String::from_utf8_lossy(&stderr).lines() {
        if line.starts_with("cargo debug runner:") {
            eprintln!("{line}");
        }
    }
    if result.is_err() {
        eprintln!(
            "compiled fixture stderr: {}",
            String::from_utf8_lossy(&stderr)
        );
    }
    result
}
