use codex_router_auth::credential_activation::{CredentialActivation, CredentialActivationRequest};
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::credential_bundle::CredentialBundle;
use codex_router_secret_store::test_support::open_encrypted_credential_store;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) const ROUTER_ADDRESS: &str = "127.0.0.1:19876";
pub(super) const UPSTREAM_ADDRESS: &str = "127.0.0.1:19877";
const MCP_ADDRESS: &str = "127.0.0.1:19878";
pub(super) const SYNTHETIC_ACCOUNT_ACCESS: &str = "synthetic-claude-access-canary";
pub(super) const SYNTHETIC_ACCOUNT_REFRESH: &str = "synthetic-claude-refresh-canary";
pub(super) const NEW_PROMPT_MARKER: &str = "CLAUDE-CLIENT-NEW-PROMPT";
pub(super) const RESUME_PROMPT_MARKER: &str = "CLAUDE-CLIENT-RESUME-PROMPT";
pub(super) const ACP_PROMPT_MARKER: &str = "CLAUDE-ACP-HOST-PROMPT";

pub(super) struct IsolatedAcceptanceRoot {
    _directory: TempDir,
    pub(super) root: PathBuf,
    pub(super) home: PathBuf,
    pub(super) codex_home: PathBuf,
    pub(super) app_server_socket: PathBuf,
    pub(super) workspace: PathBuf,
    pub(super) service_directory: PathBuf,
    pub(super) host_stdout: PathBuf,
    pub(super) host_stderr: PathBuf,
    codex_stderr: PathBuf,
    installed_codex: PathBuf,
    pub(super) settings_path: PathBuf,
}

impl IsolatedAcceptanceRoot {
    pub(super) fn create() -> io::Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("claude-routing-real-client-")
            .tempdir_in("/tmp")?;
        let root = directory.path().to_path_buf();
        let home = root.join("home");
        let codex_home = root.join("codex-home");
        let app_server_socket = root.join("native-socket/app-server.sock");
        let workspace = root.join("workspace");
        let service_directory = root.join("agent-communication");
        let host_stdout = root.join("host.stdout.log");
        let host_stderr = root.join("host.stderr.log");
        let codex_stderr = root.join("codex.app-server.stderr.log");
        let app_server_socket_parent = app_server_socket
            .parent()
            .ok_or_else(|| io::Error::other("app-server socket parent is missing"))?;
        for path in [
            home.as_path(),
            codex_home.as_path(),
            app_server_socket_parent,
            workspace.as_path(),
        ] {
            std::fs::create_dir_all(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        let managed_codex = codex_home.join("packages/standalone/current/codex");
        let managed_codex_parent = managed_codex
            .parent()
            .ok_or_else(|| io::Error::other("managed Codex parent is missing"))?;
        std::fs::create_dir_all(managed_codex_parent)?;
        let installed_codex = executable_on_path("codex")
            .ok_or_else(|| io::Error::other("installed codex executable was not found on PATH"))?;
        let installed_codex = std::fs::canonicalize(installed_codex)?;
        let codex_wrapper = concat!(
            "#!/bin/sh\n",
            "exec \"$CODEX_ROUTER_ACCEPTANCE_CODEX_BIN\" \"$@\" 2>>\"$CODEX_ROUTER_ACCEPTANCE_CODEX_STDERR\"\n"
        );
        std::fs::write(&managed_codex, codex_wrapper)?;
        std::fs::set_permissions(&managed_codex, std::fs::Permissions::from_mode(0o700))?;

        let control_socket = root.join("agent-communication/control.sock");
        let debug_profile = format!(
            "model = \"claude-routing-acceptance\"\nmodel_provider = \"codex-router-debug\"\ndefault_permissions = \"router-write-restricted\"\n\n[permissions.router-write-restricted]\nextends = \":read-only\"\n[permissions.router-write-restricted.network]\nenabled = true\nmode = \"full\"\ndomains = {{ \"*\" = \"allow\" }}\nunix_sockets = {{ \"{}\" = \"allow\" }}\n\n[permissions.router-workspace-write]\nextends = \":workspace\"\n[permissions.router-workspace-write.network]\nenabled = true\nmode = \"full\"\ndomains = {{ \"*\" = \"allow\" }}\nunix_sockets = {{ \"{}\" = \"allow\" }}\n\n[features.network_proxy]\nenabled = true\nmode = \"full\"\nproxy_url = \"http://{MCP_ADDRESS}\"\nenable_socks5 = false\nallow_upstream_proxy = false\nallow_local_binding = false\ncredential_broker = false\ndangerously_allow_all_unix_sockets = false\ndomains = {{ \"*\" = \"allow\" }}\nunix_sockets = {{ \"{}\" = \"allow\" }}\n\n[model_providers.codex-router-debug]\nname = \"Claude routing acceptance\"\nbase_url = \"http://{ROUTER_ADDRESS}/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\nsupports_websockets = true\n",
            control_socket.display(),
            control_socket.display(),
            control_socket.display(),
        );
        std::fs::write(
            codex_home.join("codex-router-debug.config.toml"),
            debug_profile,
        )?;
        let settings_path = home.join(".claude/settings.json");
        std::fs::create_dir_all(
            settings_path
                .parent()
                .ok_or_else(|| io::Error::other("Claude config directory is missing"))?,
        )?;
        std::fs::write(&settings_path, b"{}\n")?;

        Ok(Self {
            _directory: directory,
            root,
            home,
            codex_home,
            app_server_socket,
            workspace,
            service_directory,
            host_stdout,
            host_stderr,
            codex_stderr,
            installed_codex,
            settings_path,
        })
    }

    pub(super) async fn seed_synthetic_claude_account(
        &self,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let secret_root = self.root.join("secrets");
        let state = AsyncSqliteStateStore::open(&self.root.join("state.sqlite")).await?;
        let secret_store = open_encrypted_credential_store(&secret_root)?;
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs()
            .saturating_add(86_400);
        let credentials = CredentialBundle::new_claude(
            SecretString::new(SYNTHETIC_ACCOUNT_ACCESS),
            SecretString::new(SYNTHETIC_ACCOUNT_REFRESH),
            expires_at,
        )?;
        let request = CredentialActivationRequest::new(
            Provider::Claude,
            AccountId::new("acct_claude_acceptance")?,
            "Claude acceptance",
            credentials,
        );
        CredentialActivation::activate_login(&state, &secret_store, request).await?;
        drop(state);
        drop(secret_store);
        Ok(())
    }

    pub(super) fn host_command(&self) -> io::Result<tokio::process::Command> {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_codex-router"));
        command
            .args(["host", "--require-debug-isolation", "--router-root"])
            .arg(&self.root)
            .args(["--port", "19876", "--mcp-bind", MCP_ADDRESS])
            .env("HOME", &self.home)
            .env("CODEX_HOME", &self.codex_home)
            .env(
                "CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET",
                &self.app_server_socket,
            )
            .env("CODEX_ROUTER_ACCEPTANCE_CODEX_BIN", &self.installed_codex)
            .env("CODEX_ROUTER_ACCEPTANCE_CODEX_STDERR", &self.codex_stderr)
            .env(
                "CODEX_ROUTER_DEBUG_CLAUDE_UPSTREAM_BASE_URL",
                format!("http://{UPSTREAM_ADDRESS}"),
            )
            .env("OTEL_SDK_DISABLED", "true")
            .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
            .env_remove("ANTHROPIC_BASE_URL")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_CUSTOM_HEADERS")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .kill_on_drop(true)
            .stdout(Stdio::from(std::fs::File::create(&self.host_stdout)?))
            .stderr(Stdio::from(std::fs::File::create(&self.host_stderr)?));
        Ok(command)
    }

    pub(super) fn isolated_client_environment(&self) -> Vec<(String, String)> {
        vec![
            ("HOME".to_owned(), self.home.display().to_string()),
            (
                "CODEX_HOME".to_owned(),
                self.codex_home.display().to_string(),
            ),
            (
                "CODEX_ROUTER_DEBUG_ROUTER_ROOT".to_owned(),
                self.root.display().to_string(),
            ),
            (
                "CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET".to_owned(),
                self.app_server_socket.display().to_string(),
            ),
            ("OTEL_SDK_DISABLED".to_owned(), "true".to_owned()),
        ]
    }

    pub(super) fn service_manifest_path(&self) -> PathBuf {
        self.service_directory.join("service.json")
    }

    pub(super) fn settings_bytes(&self) -> io::Result<Vec<u8>> {
        std::fs::read(&self.settings_path)
    }

    pub(super) fn preserve(self) -> PathBuf {
        self._directory.keep()
    }

    pub(super) fn sanitized_host_log_tail(&self) -> io::Result<String> {
        let stdout = std::fs::read_to_string(&self.host_stdout)?;
        let stderr = std::fs::read_to_string(&self.host_stderr)?;
        let codex_stderr = std::fs::read_to_string(&self.codex_stderr).unwrap_or_default();
        if [SYNTHETIC_ACCOUNT_ACCESS, SYNTHETIC_ACCOUNT_REFRESH]
            .iter()
            .any(|canary| {
                stdout.contains(canary) || stderr.contains(canary) || codex_stderr.contains(canary)
            })
        {
            return Ok(
                "isolated Host logs contained a credential canary; content withheld".to_owned(),
            );
        }
        let tail = stdout
            .lines()
            .chain(stderr.lines())
            .chain(codex_stderr.lines())
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        Ok(tail)
    }
}

pub(super) struct HostProcess {
    child: tokio::process::Child,
}

impl HostProcess {
    pub(super) async fn start(root: &IsolatedAcceptanceRoot) -> io::Result<Self> {
        let child = root.host_command()?.spawn()?;
        Ok(Self { child })
    }

    pub(super) async fn wait_until_ready(
        &mut self,
        root: &IsolatedAcceptanceRoot,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(format!("isolated Host exited before readiness: {status}").into());
            }
            if let Ok(bytes) = tokio::fs::read(root.service_manifest_path()).await
                && let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes)
                && router_health_is_ready(ROUTER_ADDRESS).await
            {
                let endpoint = manifest
                    .get("routerProxyEndpoint")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("Host did not publish routerProxyEndpoint")?;
                if endpoint != ROUTER_ADDRESS {
                    return Err(
                        format!("Host published unexpected Router endpoint {endpoint}").into(),
                    );
                }
                let token_file = root.root.join("secrets/local_router_token.secret");
                if !token_file.is_file() || !root.root.join("secrets/.token.lock").is_file() {
                    return Err("Host readiness did not publish its locked local token".into());
                }
                return Ok(manifest);
            }
            if Instant::now() >= deadline {
                return Err("isolated Host did not publish readiness before its deadline".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub(super) async fn stop(mut self) -> Result<ExitStatus, Box<dyn std::error::Error>> {
        send_interrupt(self.child.id())?;
        let status = tokio::time::timeout(Duration::from_secs(20), self.child.wait())
            .await
            .map_err(|_| "isolated Host did not stop after SIGINT")??;
        Ok(status)
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        let Some(pid) = self.child.id() else {
            return;
        };
        let _signal = send_interrupt(Some(pid));
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _forced_stop = self.child.start_kill();
        let _waited = self.child.try_wait();
    }
}

async fn router_health_is_ready(address: &str) -> bool {
    let Ok(mut stream) = tokio::net::TcpStream::connect(address).await else {
        return false;
    };
    let request = format!("GET /healthz HTTP/1.1\r\nhost: {address}\r\nconnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut status = [0_u8; 128];
    let Ok(count) = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut status)).await
    else {
        return false;
    };
    count.is_ok_and(|count| {
        status.get(..count).is_some_and(|status_line| {
            String::from_utf8_lossy(status_line).starts_with("HTTP/1.1 200")
        })
    })
}

fn send_interrupt(pid: Option<u32>) -> io::Result<()> {
    let Some(pid) = pid else {
        return Ok(());
    };
    let status = Command::new("/bin/kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("failed to signal isolated Host"))
    }
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}
