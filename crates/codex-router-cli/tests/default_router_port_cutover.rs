#[cfg(not(feature = "keychain-test-support"))]
compile_error!(
    "default Router port cutover requires keychain-test-support so the compiled CLI cannot use the production Keychain adapter"
);

use codex_router_secret_store::{SecretStore, file_backend::FileSecretStore, model::SecretKey};
use reqwest::{Client, StatusCode, header::CONNECTION};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tokio::process::{Child, Command as TokioCommand};

const PRODUCTION_ROUTER_PORT: u16 = 19741;
const CUTOVER_CONNECTIONS_PER_PHASE: &str = "3";
const PORTS_RESERVED_FOR_OTHER_ROUTER_SURFACES: [u16; 4] =
    [8787, 18787, 43127, PRODUCTION_ROUTER_PORT];

#[tokio::test]
async fn compiled_cli_default_router_port_cutover_uses_isolated_state() -> Result<(), Box<dyn Error>>
{
    let fixture_directory = tempfile::tempdir()?;
    let fixture_root = fixture_directory.path();
    let codex_home = fixture_root.join("codex-home");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_codex-router"));

    // Fail closed if another process already owns the new default. This test never
    // inspects, signals, or replaces an existing listener.
    require_loopback_port_free(PRODUCTION_ROUTER_PORT)?;
    let old_port = choose_isolated_old_port()?;

    let old_profile = write_profile(&binary, &codex_home, Some(old_port))?;
    let old_endpoint = format!("http://127.0.0.1:{old_port}/v1");
    let readback_old_endpoint = read_router_profile_base_url(&old_profile)?;
    if readback_old_endpoint != old_endpoint {
        return Err(format!(
            "old profile endpoint mismatch: expected {old_endpoint}, read {readback_old_endpoint}"
        )
        .into());
    }

    let old_phase = run_serve_phase(&binary, fixture_root, Some(old_port)).await?;
    require_loopback_port_free(old_port)?;

    let new_profile = write_profile(&binary, &codex_home, None)?;
    let new_endpoint = format!("http://127.0.0.1:{PRODUCTION_ROUTER_PORT}/v1");
    let readback_new_endpoint = read_router_profile_base_url(&new_profile)?;
    if readback_new_endpoint != new_endpoint {
        return Err(format!(
            "new profile endpoint mismatch: expected {new_endpoint}, read {readback_new_endpoint}"
        )
        .into());
    }
    let expected_new_profile = old_profile.replace(&old_endpoint, &new_endpoint);
    if new_profile != expected_new_profile {
        return Err("profile cutover changed settings beyond the base_url endpoint".into());
    }
    let old_profile_sha256 = format!("{:x}", Sha256::digest(old_profile.as_bytes()));
    let new_profile_sha256 = format!("{:x}", Sha256::digest(new_profile.as_bytes()));

    let new_phase = run_serve_phase(&binary, fixture_root, None).await?;
    require_loopback_port_free(old_port)?;
    require_loopback_port_free(PRODUCTION_ROUTER_PORT)?;

    println!(
        "isolated Router cutover: old_port={old_port} old_pid={} old_profile_sha256={} old_health={} old_unauthenticated={} old_authenticated={} old_listener_released=true; new_port={PRODUCTION_ROUTER_PORT} new_pid={} new_profile_sha256={new_profile_sha256} new_health={} new_unauthenticated={} new_authenticated={} final_old_port_free=true final_new_port_free=true",
        old_phase.process_id,
        old_profile_sha256,
        old_phase.health_status,
        old_phase.unauthenticated_status,
        old_phase.authenticated_status,
        new_phase.process_id,
        new_phase.health_status,
        new_phase.unauthenticated_status,
        new_phase.authenticated_status,
    );
    Ok(())
}

struct ServePhaseReceipt {
    process_id: u32,
    health_status: u16,
    unauthenticated_status: u16,
    authenticated_status: u16,
}

async fn run_serve_phase(
    binary: &Path,
    fixture_root: &Path,
    explicit_port: Option<u16>,
) -> Result<ServePhaseReceipt, Box<dyn Error>> {
    let port = explicit_port.unwrap_or(PRODUCTION_ROUTER_PORT);
    let mut command = TokioCommand::new(binary);
    command
        .args([
            "serve",
            "--state-db",
            fixture_root
                .join("state.sqlite")
                .to_str()
                .ok_or("state path is not UTF-8")?,
            "--secret-root",
            fixture_root
                .join("secrets")
                .to_str()
                .ok_or("secret path is not UTF-8")?,
            "--disable-background-quota-refresh",
            "--require-local-token",
            "--max-connections",
            CUTOVER_CONNECTIONS_PER_PHASE,
        ])
        .env("HOME", fixture_root)
        .env("OTEL_SDK_DISABLED", "true")
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .env_remove("CODEX_ROUTER_DEBUG_ROUTER_ROOT")
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(explicit_port) = explicit_port {
        command.args(["--listen-host", "127.0.0.1", "--port"]);
        command.arg(explicit_port.to_string());
    }

    let mut child = command.spawn()?;
    let process_id = child.id().ok_or("serve child has no process id")?;
    let client = Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(3))
        .build()?;
    let health_status = wait_for_health(&client, &mut child, port).await?;
    if health_status != StatusCode::OK {
        return Err(format!("GET /healthz returned {health_status} on {port}").into());
    }

    let unauthenticated_status = client
        .post(format!("http://127.0.0.1:{port}/v1/responses"))
        .header(CONNECTION, "close")
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"port-cutover-auth-probe"}"#)
        .send()
        .await?
        .status();
    if unauthenticated_status != StatusCode::UNAUTHORIZED {
        return Err(format!(
            "unauthenticated POST /v1/responses returned {unauthenticated_status} on {port}"
        )
        .into());
    }

    let token = read_local_router_token(&fixture_root.join("secrets"))?;
    let authenticated_status = client
        .post(format!("http://127.0.0.1:{port}/v1/responses"))
        .header(CONNECTION, "close")
        .header("content-type", "application/json")
        .header("X-Codex-Router-Token", token)
        .body(r#"{"model":"gpt-5","input":"port-cutover-auth-probe"}"#)
        .send()
        .await?
        .status();
    if authenticated_status != StatusCode::SERVICE_UNAVAILABLE {
        return Err(format!(
            "valid local Router token without any configured accounts returned {authenticated_status} on {port}"
        )
        .into());
    }

    let exit_status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await??;
    if !exit_status.success() {
        return Err(format!("serve process {process_id} exited with {exit_status}").into());
    }

    Ok(ServePhaseReceipt {
        process_id,
        health_status: health_status.as_u16(),
        unauthenticated_status: unauthenticated_status.as_u16(),
        authenticated_status: authenticated_status.as_u16(),
    })
}

async fn wait_for_health(
    client: &Client,
    child: &mut Child,
    port: u16,
) -> Result<StatusCode, Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("serve exited before health on {port}: {status}").into());
        }

        match client
            .get(format!("http://127.0.0.1:{port}/healthz"))
            .header(CONNECTION, "close")
            .send()
            .await
        {
            Ok(response) => return Ok(response.status()),
            Err(error) if error.is_connect() && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn write_profile(
    binary: &Path,
    codex_home: &Path,
    port: Option<u16>,
) -> Result<String, Box<dyn Error>> {
    let mut command = Command::new(binary);
    command
        .args(["profile", "write", "--codex-home"])
        .arg(codex_home)
        .arg("--approve-codex-home-write");
    if let Some(port) = port {
        command.args(["--port", &port.to_string()]);
    }

    let output = command
        .env(
            "HOME",
            codex_home.parent().ok_or("Codex home has no parent")?,
        )
        .env("OTEL_SDK_DISABLED", "true")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "profile write failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(std::fs::read_to_string(
        codex_home.join("codex-router.config.toml"),
    )?)
}

fn read_local_router_token(secret_root: &Path) -> Result<String, Box<dyn Error>> {
    let store = FileSecretStore::open(secret_root)?;
    let key = SecretKey::new("local_router_token")?;
    Ok(store.read_secret(&key)?.expose_secret().to_owned())
}

fn read_router_profile_base_url(profile_contents: &str) -> Result<String, Box<dyn Error>> {
    let profile: toml::Table = toml::from_str(profile_contents)?;
    let providers = profile
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| std::io::Error::other("profile omitted model_providers table"))?;
    let router_provider = providers
        .get("codex-router")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| std::io::Error::other("profile omitted codex-router provider"))?;
    let base_url = router_provider
        .get("base_url")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| std::io::Error::other("profile omitted string base_url"))?;
    Ok(base_url.to_owned())
}

fn choose_isolated_old_port() -> Result<u16, Box<dyn Error>> {
    for _attempt in 0..16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        if !PORTS_RESERVED_FOR_OTHER_ROUTER_SURFACES.contains(&port) {
            return Ok(port);
        }
    }
    Err("could not choose an isolated old port".into())
}

fn require_loopback_port_free(port: u16) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("test fixture requires unused loopback port {port}: {error}"),
        )
    })?;
    drop(listener);
    Ok(())
}
