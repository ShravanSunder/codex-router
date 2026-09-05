//! Single-source router profile projections for upstream Codex.

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
    #[must_use]
    pub fn root_overrides(self) -> Vec<String> {
        vec![
            "model_provider=\"codex-router\"".to_owned(),
            "model_providers.codex-router.name=\"codex-router\"".to_owned(),
            format!(
                "model_providers.codex-router.base_url=\"http://127.0.0.1:{}/v1\"",
                self.port
            ),
            "model_providers.codex-router.wire_api=\"responses\"".to_owned(),
            "model_providers.codex-router.requires_openai_auth=false".to_owned(),
            "model_providers.codex-router.supports_websockets=true".to_owned(),
        ]
    }
}
