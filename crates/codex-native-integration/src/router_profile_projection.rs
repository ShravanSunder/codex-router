//! Single-source router profile projections for upstream Codex.

use std::path::Path;
use std::path::PathBuf;

/// Permission profiles Router selects for its own threads, with the native parent each extends.
const ROUTER_PERMISSION_PROFILES: [(&str, &str); 2] = [
    ("router-write-restricted", ":read-only"),
    ("router-workspace-write", ":workspace"),
];

/// The exact Control socket file the managed app-server is allowed to reach.
///
/// Held as an absolute path that is representable as a quoted TOML key, so the
/// allowance cannot silently widen to a directory or to every Unix socket.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouterControlSocketPath(PathBuf);

#[derive(Debug, thiserror::Error)]
pub enum RouterControlSocketError {
    #[error("the collaboration service directory must be an absolute path")]
    RelativeCollaborationDirectory,
    #[error("the Router control socket path is not representable as a configuration key")]
    UnrepresentablePath,
}

impl RouterControlSocketPath {
    /// Resolves the canonical `control.sock` inside one collaboration service directory.
    pub fn in_collaboration_directory(
        collaboration_directory: &Path,
    ) -> Result<Self, RouterControlSocketError> {
        if !collaboration_directory.is_absolute() {
            return Err(RouterControlSocketError::RelativeCollaborationDirectory);
        }
        let socket = collaboration_directory.join("control.sock");
        let text = socket
            .to_str()
            .ok_or(RouterControlSocketError::UnrepresentablePath)?;
        // Fail closed instead of escaping: a quoted basic TOML key must stay literal.
        if text.contains('\0') || text.contains('"') || text.contains('\\') {
            return Err(RouterControlSocketError::UnrepresentablePath);
        }
        Ok(Self(socket))
    }

    /// Returns the socket file the allowance names.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Renders the single-entry allow map used by every network table.
    fn allow_entry(&self) -> String {
        format!("{{\"{}\"=\"allow\"}}", self.0.display())
    }
}

/// Codex profile routing model traffic through the loopback router.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodexRouterProfile {
    port: u16,
}

impl CodexRouterProfile {
    /// Creates the projection for one loopback router port.
    #[must_use]
    pub const fn new(port: u16) -> Self {
        Self { port }
    }

    /// Renders the profile file used by existing CLI commands.
    #[must_use]
    pub fn render(self) -> String {
        format!(
            r#"model_provider = "codex-router"

[model_providers.codex-router]
name = "codex-router"
base_url = "http://127.0.0.1:{}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = true
"#,
            self.port
        )
    }

    /// Returns root configuration overrides for the managed app-server child.
    ///
    /// Carries both halves Router owns: the model provider, and the network
    /// capability that lets Router-launched threads reach the Control socket.
    /// Per-session overrides add only dotted `extends` and `filesystem` keys,
    /// so they merge into these profiles instead of replacing their `network`.
    #[must_use]
    pub fn root_overrides(self, control_socket: &RouterControlSocketPath) -> Vec<String> {
        let mut overrides = vec![
            "model_provider=\"codex-router\"".to_owned(),
            "model_providers.codex-router.name=\"codex-router\"".to_owned(),
            format!(
                "model_providers.codex-router.base_url=\"http://127.0.0.1:{}/v1\"",
                self.port
            ),
            "model_providers.codex-router.wire_api=\"responses\"".to_owned(),
            "model_providers.codex-router.requires_openai_auth=false".to_owned(),
            "model_providers.codex-router.supports_websockets=true".to_owned(),
        ];
        overrides.extend(network_overrides("features.network_proxy", control_socket));
        for setting in [
            "allow_local_binding",
            "dangerously_allow_all_unix_sockets",
            "enable_socks5",
            "allow_upstream_proxy",
            "credential_broker",
        ] {
            overrides.push(format!("features.network_proxy.{setting}=false"));
        }
        for (profile, parent) in ROUTER_PERMISSION_PROFILES {
            overrides.push(format!("permissions.{profile}.extends=\"{parent}\""));
            overrides.extend(network_overrides(
                &format!("permissions.{profile}.network"),
                control_socket,
            ));
        }
        overrides
    }
}

/// Projects one network table: enabled in full mode, every host, exactly one socket.
fn network_overrides(prefix: &str, control_socket: &RouterControlSocketPath) -> Vec<String> {
    vec![
        format!("{prefix}.enabled=true"),
        format!("{prefix}.mode=\"full\""),
        format!("{prefix}.domains={{\"*\"=\"allow\"}}"),
        format!("{prefix}.unix_sockets={}", control_socket.allow_entry()),
    ]
}
