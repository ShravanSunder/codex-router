use codex_native_integration::{
    AppServerCommandSpec, CodexPaths, CodexRouterProfile, DebugCodexProfile, NativeConnectionError,
    NativeProtocolConnection,
};
use codex_router_core::{ids::AccountId, provider::Provider};
use codex_router_secret_store::{
    SecretStore,
    account_tokens::{AccountCredentialBundle, openai_account_credential_bundle_key},
};
use codex_router_state::{
    account::{AccountRecord, AccountStatus},
    quota_snapshot::{PersistedSelectorQuotaWindow, SelectorQuotaWindowStatus},
    repositories::{AccountStateRepository, SelectorQuotaRepository},
    sqlite::SqliteStateStore,
};
use std::{
    error::Error,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
};

pub const SYNTHETIC_POOL_TOKEN: &str = "native-compaction-synthetic-pool";

#[derive(Clone, Copy, Debug)]
pub enum FixtureProfile {
    Production,
    Debug,
}

pub struct NativeFixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    socket: PathBuf,
    router: Child,
    native: Child,
    version: String,
    address: SocketAddr,
    native_pid: u32,
    router_pid: u32,
}

impl NativeFixture {
    pub async fn start(
        profile: FixtureProfile,
        legacy: bool,
        upstream: SocketAddr,
    ) -> Result<Self, Box<dyn Error>> {
        let installed = std::env::var_os("CODEX_ROUTER_NATIVE_PROOF_CODEX_BIN")
            .map(PathBuf::from)
            .ok_or("CODEX_ROUTER_NATIVE_PROOF_CODEX_BIN must identify installed Codex")?;
        let version_output = Command::new(&installed).arg("--version").output().await?;
        assert!(version_output.status.success());
        let version = String::from_utf8(version_output.stdout)?.trim().to_owned();
        assert!(
            version.starts_with("codex-cli 0.160."),
            "unsupported native protocol version: {version}"
        );
        let root = tempfile::Builder::new()
            .prefix("native-compaction-")
            .tempdir_in("/tmp")?;
        let home = root.path().join("home");
        let codex_home = root.path().join("codex");
        let workspace = root.path().join("workspace");
        let socket = root.path().join("socket/native.sock");
        for path in [
            &home,
            &codex_home,
            &workspace,
            socket.parent().ok_or("missing socket parent")?,
        ] {
            std::fs::create_dir_all(path)?;
        }
        let state = root.path().join("state.sqlite");
        let secrets = root.path().join("secrets");
        seed_pool(&state, &secrets)?;
        // The public CLI requires a nonzero port; reserve an OS-assigned loopback port.
        let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = reservation.local_addr()?.port().to_string();
        drop(reservation);
        let mut router = Command::new(env!("CARGO_BIN_EXE_codex-router"))
            .args([
                "serve",
                "--port",
                &port,
                "--listen-host",
                "127.0.0.1",
                "--state-db",
            ])
            .arg(&state)
            .arg("--secret-root")
            .arg(&secrets)
            .args([
                "--upstream-base-url",
                &format!("http://{upstream}/v1"),
                "--now-unix-seconds",
                "1030",
                "--max-snapshot-age-seconds",
                "60",
                "--disable-background-quota-refresh",
            ])
            .env_clear()
            .env("HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(std::fs::File::create(
                root.path().join("router.stderr"),
            )?))
            .kill_on_drop(true)
            .spawn()?;
        let router_pid = router.id().ok_or("missing Router PID")?;
        let setup = async {
        let stdout = router.stdout.take().ok_or("missing Router stdout")?;
        let mut lines = BufReader::new(stdout).lines();
        let address = tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(line) = lines.next_line().await? {
                if let Some(address) = line.strip_prefix("listening: ") { return Ok::<SocketAddr, Box<dyn Error>>(address.parse()?); }
            }
            Err(format!("Router exited without readiness: {}", std::fs::read_to_string(root.path().join("router.stderr"))?).into())
        }).await??;
        let projection = CodexRouterProfile::new(address.port());
        let paths = CodexPaths::from_codex_home(codex_home.clone());
        let mut spec = AppServerCommandSpec::new(&paths, &projection, &socket).with_remote_control(false);
        let provider_key = match profile { FixtureProfile::Production => "codex-router", FixtureProfile::Debug => "codex-router-debug" };
        match profile {
            FixtureProfile::Production => { std::fs::write(codex_home.join("codex-router.config.toml"), projection.render())?; }
            FixtureProfile::Debug => {
                let text = format!("model_provider = \"codex-router-debug\"\n[model_providers.codex-router-debug]\nname = \"OpenAI\"\nbase_url = \"http://{address}/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\nsupports_websockets = true\nstream_max_retries = 1\n[features]\nenable_request_compression = false\n");
                std::fs::write(codex_home.join("codex-router-debug.config.toml"), &text)?;
                spec = spec.with_debug_profile(&DebugCodexProfile::parse(&text, address.port())?);
            }
        }
        let mut command = Command::new(&installed);
        command.args(["-c", "model=\"gpt-5.4\"", "-c", "approval_policy=\"never\""])
            .arg("-c").arg(format!("model_providers.{provider_key}.stream_max_retries=1"));
        // Native root overrides come first; the explicit legacy control is applied last.
        let mut arguments = spec.arguments();
        if legacy { let position = arguments.iter().position(|argument| argument == "app-server").ok_or("missing app-server argument")?; arguments.splice(position..position, ["-c".into(), format!("model_providers.{provider_key}.name=\"codex-router\"").into()]); }
        command.args(arguments).env_clear().env("HOME", &home).env("CODEX_HOME", &codex_home)
            .env("OPENAI_API_KEY", "native-compaction-synthetic-client")
            .env("CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED", "1")
            .env("OTEL_SDK_DISABLED", "true")
            .stdin(Stdio::null()).stdout(Stdio::null())
            .stderr(Stdio::from(std::fs::File::create(root.path().join("native.stderr"))?))
            .kill_on_drop(true);
        let native = command.spawn()?;
        Ok::<(Child, SocketAddr), Box<dyn Error>>((native, address))
        }.await;
        let (native, address) = match setup {
            Ok(value) => value,
            Err(error) => {
                stop_child(&mut router, router_pid).await?;
                return Err(error);
            }
        };
        let native_pid = native.id().ok_or("missing native PID")?;
        Ok(Self {
            _root: root,
            workspace,
            socket,
            router,
            native,
            version,
            address,
            router_pid,
            native_pid,
        })
    }
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub async fn connect(&mut self) -> Result<NativeProtocolConnection, Box<dyn Error>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.native.try_wait()? {
                return Err(format!(
                    "native app-server exited before connect: {status}; {}",
                    std::fs::read_to_string(self._root.path().join("native.stderr"))?
                )
                .into());
            }
            if self.socket.exists() {
                match NativeProtocolConnection::connect(&self.socket).await {
                    Ok(client) => return Ok(client),
                    Err(NativeConnectionError::Unavailable) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("native socket readiness timed out before dispatch".into());
            }
            tokio::task::yield_now().await;
        }
    }
    pub async fn stop(&mut self) -> Result<(), Box<dyn Error>> {
        let native_result = stop_child(&mut self.native, self.native_pid).await;
        let router_result = stop_child(&mut self.router, self.router_pid).await;
        native_result?;
        router_result?;
        if tokio::net::TcpStream::connect(self.address).await.is_ok() {
            return Err("owned Router listener survived cleanup".into());
        }
        if tokio::net::UnixStream::connect(&self.socket).await.is_ok() {
            return Err("owned native socket listener survived cleanup".into());
        }
        if self.socket.exists() {
            std::fs::remove_file(&self.socket)?;
        }
        if self.socket.exists() {
            return Err("owned native socket survived cleanup".into());
        }
        eprintln!(
            "native_compaction_cleanup children_reaped=true router_listener_absent=true native_socket_absent=true"
        );
        Ok(())
    }
}

fn seed_pool(state_path: &Path, secret_root: &Path) -> Result<(), Box<dyn Error>> {
    let state = SqliteStateStore::open(state_path)?;
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)?;
    let account_id = AccountId::new("acct_native_compaction_fixture")?;
    let account = AccountRecord::new(
        Provider::Openai,
        account_id.clone(),
        "native compaction fixture",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    AccountStateRepository::upsert_account(&state, &account)?;
    let windows: Vec<_> = [18_000, 604_800]
        .into_iter()
        .map(|duration| {
            PersistedSelectorQuotaWindow::new(
                account_id.clone(),
                "responses",
                duration,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(100)
            .with_reset_unix_seconds(1000 + duration)
            .with_effective(true)
            .with_observed_unix_seconds(1000)
        })
        .collect();
    SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        &state,
        &account_id,
        "responses",
        &windows,
        1000,
        2000,
    )?;
    let key = openai_account_credential_bundle_key(&account_id, 1)?;
    let bundle = AccountCredentialBundle::imported_codex_auth(SYNTHETIC_POOL_TOKEN, None)
        .to_secret_string()?;
    secrets.write_secret(&key, &bundle)?;
    Ok(())
}

async fn stop_child(child: &mut Child, owned_pid: u32) -> Result<(), Box<dyn Error>> {
    let pid = i32::try_from(owned_pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or("invalid owned child PID")?;
    if child.try_wait()?.is_none() {
        child.kill().await?;
    }
    child.wait().await?;
    {
        if !matches!(
            rustix::process::test_kill_process(pid),
            Err(rustix::io::Errno::SRCH)
        ) {
            return Err("owned fixture child absence not confirmed".into());
        }
    }
    Ok(())
}
