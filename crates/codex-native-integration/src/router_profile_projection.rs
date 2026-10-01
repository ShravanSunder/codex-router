//! Single-source router profile projections for upstream Codex.

/// Permission profiles Router selects for its own threads, with the native parent each extends.
const ROUTER_PERMISSION_PROFILES: [(&str, &str); 2] = [
    ("router-write-restricted", ":read-only"),
    ("router-workspace-write", ":workspace"),
];

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
    pub fn root_overrides(self) -> Vec<String> {
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
        overrides.extend(router_permission_profile_overrides());
        overrides
    }
}

/// Returns the network half of Router's permission profiles and the app-server's
/// default sandbox, shared by the production and debug app-server launches.
///
/// Codex refuses to start an app-server that defines permission profiles without a
/// default, so Router supplies `sandbox_mode="workspace-write"` itself rather than
/// relying on the owner's home configuration. It is only the default: Router's own
/// threads select a profile, and a client that asks for another sandbox on start
/// (for example `--yolo`, which requests `danger-full-access`) still gets it.
///
/// Both profiles get direct network access. The managed network proxy is switched
/// off explicitly so a home or project setting cannot reinstate it: behind it,
/// Seatbelt admits only the loopback proxy and allowlisted Unix sockets, which breaks
/// DNS for SwiftPM and ssh and the 1Password agent. Per-session overrides add only
/// dotted `extends` and `filesystem` keys, so they merge into these profiles instead
/// of replacing their `network`.
#[must_use]
pub fn router_permission_profile_overrides() -> Vec<String> {
    let mut overrides = vec![
        "sandbox_mode=\"workspace-write\"".to_owned(),
        "features.network_proxy.enabled=false".to_owned(),
    ];
    for (profile, parent) in ROUTER_PERMISSION_PROFILES {
        overrides.push(format!("permissions.{profile}.extends=\"{parent}\""));
        overrides.push(format!("permissions.{profile}.network.enabled=true"));
    }
    overrides
}
