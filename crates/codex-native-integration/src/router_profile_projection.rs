//! Single-source router profile projections for upstream Codex.

use std::path::Path;
use std::path::PathBuf;

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
requires_openai_auth = true
supports_websockets = true
"#,
            self.port
        )
    }

    /// Returns root configuration overrides for the managed app-server child:
    /// the model provider, then Router's permission profiles.
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
            "model_providers.codex-router.requires_openai_auth=true".to_owned(),
            "model_providers.codex-router.supports_websockets=true".to_owned(),
        ];
        overrides.extend(router_permission_profile_overrides(control_socket));
        overrides
    }
}

/// Returns the network half of Router's permission profiles, shared by the production
/// and debug app-server launches.
///
/// `router-workspace-write` gets direct network: with the managed proxy off, Seatbelt
/// allows DNS, ssh, SwiftPM and local sockets such as the 1Password agent.
/// `router-write-restricted` keeps the proxied shape: all public hosts in full mode
/// through Codex's managed proxy and only the Control socket, so agents that read
/// untrusted content stay behind a real boundary. The proxy feature is configured
/// here but switched off; write-restricted sessions switch it on in their own
/// per-session overrides, because the proxy starts for every network-enabled profile
/// whose thread has the feature on. Per-session overrides add only dotted keys, so
/// they merge into these tables instead of replacing them.
#[must_use]
pub fn router_permission_profile_overrides(
    control_socket: &RouterControlSocketPath,
) -> Vec<String> {
    let mut overrides = network_proxy_overrides("features.network_proxy", false, control_socket);
    for setting in [
        "allow_local_binding",
        "dangerously_allow_all_unix_sockets",
        "enable_socks5",
        "allow_upstream_proxy",
        "credential_broker",
    ] {
        overrides.push(format!("features.network_proxy.{setting}=false"));
    }
    overrides.push("permissions.router-write-restricted.extends=\":read-only\"".to_owned());
    overrides.extend(network_proxy_overrides(
        "permissions.router-write-restricted.network",
        true,
        control_socket,
    ));
    overrides.push("permissions.router-workspace-write.extends=\":workspace\"".to_owned());
    overrides.push("permissions.router-workspace-write.network.enabled=true".to_owned());
    overrides
}

/// Projects one proxied network table: full mode, every host, exactly one socket.
fn network_proxy_overrides(
    prefix: &str,
    enabled: bool,
    control_socket: &RouterControlSocketPath,
) -> Vec<String> {
    vec![
        format!("{prefix}.enabled={enabled}"),
        format!("{prefix}.mode=\"full\""),
        format!("{prefix}.domains={{\"*\"=\"allow\"}}"),
        format!("{prefix}.unix_sockets={}", control_socket.allow_entry()),
    ]
}
