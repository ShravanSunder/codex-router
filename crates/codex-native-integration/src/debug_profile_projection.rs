//! Read-only debug configuration projected through supported native server overrides.
use std::{io::Read, path::Path};

const MAX_PROFILE_BYTES: usize = 64 * 1024;

pub struct DebugCodexProfile {
    overrides: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DebugProfileError {
    #[error("debug Codex profile could not be read")]
    Unavailable,
    #[error("debug Codex profile exceeds the size limit")]
    TooLarge,
    #[error("debug Codex profile is malformed")]
    Malformed,
    #[error("debug Codex profile contains settings unsupported by the backend projection")]
    UnsupportedSettings,
    #[error("debug Codex profile must select its debug provider at the chosen loopback debug port")]
    RoutingMismatch,
}

impl DebugCodexProfile {
    pub fn read(codex_home: &Path, port: u16) -> Result<Self, DebugProfileError> {
        let file = std::fs::File::open(codex_home.join("codex-router-debug.config.toml"))
            .map_err(|_| DebugProfileError::Unavailable)?;
        let mut text = String::new();
        file.take((MAX_PROFILE_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|_| DebugProfileError::Unavailable)?;
        Self::parse(&text, port)
    }

    pub fn parse(text: &str, port: u16) -> Result<Self, DebugProfileError> {
        if text.len() > MAX_PROFILE_BYTES {
            return Err(DebugProfileError::TooLarge);
        }
        let table: toml::Table = toml::from_str(text).map_err(|_| DebugProfileError::Malformed)?;
        for (key, value) in &table {
            match key.as_str() {
                "model"
                | "model_reasoning_effort"
                | "model_reasoning_summary"
                | "model_verbosity"
                | "model_provider" => {
                    if !bounded_string(value) {
                        return Err(DebugProfileError::UnsupportedSettings);
                    }
                }
                "model_providers" => {}
                "projects" => validate_projects(value)?,
                _ => return Err(DebugProfileError::UnsupportedSettings),
            }
        }
        if port == 0
            || port == 8787
            || table.get("model_provider").and_then(toml::Value::as_str)
                != Some("codex-router-debug")
        {
            return Err(DebugProfileError::RoutingMismatch);
        }
        let providers = table
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .filter(|providers| providers.len() == 1)
            .ok_or(DebugProfileError::RoutingMismatch)?;
        let provider = providers
            .get("codex-router-debug")
            .and_then(toml::Value::as_table)
            .ok_or(DebugProfileError::RoutingMismatch)?;
        for (key, value) in provider {
            let valid = match key.as_str() {
                "name" | "base_url" | "wire_api" => bounded_string(value),
                "requires_openai_auth" | "supports_websockets" => value.as_bool().is_some(),
                "stream_max_retries"
                | "request_max_retries"
                | "stream_idle_timeout_ms"
                | "websocket_connect_timeout_ms" => {
                    value.as_integer().is_some_and(|value| value >= 0)
                }
                // Literal credentials and unsupported transport behavior must not enter argv.
                _ => false,
            };
            if !valid {
                return Err(DebugProfileError::UnsupportedSettings);
            }
        }
        let endpoint = format!("http://127.0.0.1:{port}/v1");
        if provider.get("base_url").and_then(toml::Value::as_str) != Some(endpoint.as_str())
            || provider.get("wire_api").and_then(toml::Value::as_str) != Some("responses")
            || provider
                .get("requires_openai_auth")
                .and_then(toml::Value::as_bool)
                != Some(false)
            || provider
                .get("supports_websockets")
                .and_then(toml::Value::as_bool)
                != Some(true)
        {
            return Err(DebugProfileError::RoutingMismatch);
        }
        Ok(Self {
            overrides: table
                .into_iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
        })
    }

    pub(crate) fn root_overrides(&self) -> Vec<String> {
        self.overrides.clone()
    }
}

fn bounded_string(value: &toml::Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| !text.is_empty() && text.len() <= 4096 && !text.contains('\0'))
}

fn validate_projects(value: &toml::Value) -> Result<(), DebugProfileError> {
    let projects = value
        .as_table()
        .ok_or(DebugProfileError::UnsupportedSettings)?;
    for (path, project) in projects {
        if !Path::new(path).is_absolute() || path.contains('\0') {
            return Err(DebugProfileError::UnsupportedSettings);
        }
        let project = project
            .as_table()
            .filter(|project| project.len() == 1)
            .ok_or(DebugProfileError::UnsupportedSettings)?;
        if !matches!(
            project.get("trust_level").and_then(toml::Value::as_str),
            Some("trusted" | "untrusted")
        ) {
            return Err(DebugProfileError::UnsupportedSettings);
        }
    }
    Ok(())
}
