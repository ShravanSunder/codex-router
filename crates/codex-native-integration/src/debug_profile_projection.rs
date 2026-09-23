//! Read-only debug configuration projected through supported native server overrides.
use std::{collections::BTreeSet, io::Read, path::Path};

const MAX_PROFILE_BYTES: usize = 64 * 1024;
const LOCAL_ONLY_KEYS: [&str; 6] = [
    "approval_policy",
    "approvals_reviewer",
    "auto_review",
    "apps",
    // The TUI can persist these into the selected profile; neither belongs in backend argv.
    "notice",
    "tui",
];

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
            if LOCAL_ONLY_KEYS.contains(&key.as_str()) {
                tracing::warn!(setting = key, "dropping local-only debug profile setting");
                continue;
            }
            match key.as_str() {
                "model"
                | "model_reasoning_effort"
                | "model_reasoning_summary"
                | "model_verbosity"
                | "model_provider"
                | "default_permissions" => {
                    if !bounded_string(value) {
                        return Err(DebugProfileError::UnsupportedSettings);
                    }
                }
                "model_providers" => {}
                "projects" => validate_projects(value)?,
                "permissions" | "features" => {}
                _ => return Err(DebugProfileError::UnsupportedSettings),
            }
        }
        validate_network_experiment_configuration(&table)?;
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
                .is_none()
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
                .filter(|(key, _)| !LOCAL_ONLY_KEYS.contains(&key.as_str()))
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

fn validate_network_experiment_configuration(table: &toml::Table) -> Result<(), DebugProfileError> {
    let features = match table.get("features") {
        Some(value) => {
            let features = value
                .as_table()
                .filter(|features| {
                    exact_keys(features, &["image_generation"])
                        || exact_keys(features, &["network_proxy"])
                        || exact_keys(features, &["image_generation", "network_proxy"])
                })
                .ok_or(DebugProfileError::UnsupportedSettings)?;
            if features
                .get("image_generation")
                .is_some_and(|value| value.as_bool().is_none())
            {
                return Err(DebugProfileError::UnsupportedSettings);
            }
            Some(features)
        }
        None => None,
    };
    let network_requested = table.contains_key("default_permissions")
        || table.contains_key("permissions")
        || features.is_some_and(|features| features.contains_key("network_proxy"));
    if !network_requested {
        return Ok(());
    }
    if table
        .get("default_permissions")
        .and_then(toml::Value::as_str)
        != Some("router-write-restricted")
    {
        return Err(DebugProfileError::UnsupportedSettings);
    }
    let permissions = table
        .get("permissions")
        .and_then(toml::Value::as_table)
        .filter(|permissions| {
            exact_keys(
                permissions,
                &["router-workspace-write", "router-write-restricted"],
            )
        })
        .ok_or(DebugProfileError::UnsupportedSettings)?;
    let restricted_socket =
        validate_permission_profile(permissions, "router-write-restricted", ":read-only")?;
    let workspace_socket =
        validate_permission_profile(permissions, "router-workspace-write", ":workspace")?;
    if restricted_socket != workspace_socket {
        return Err(DebugProfileError::UnsupportedSettings);
    }
    let features = features.ok_or(DebugProfileError::UnsupportedSettings)?;
    let proxy = features
        .get("network_proxy")
        .and_then(toml::Value::as_table)
        .filter(|proxy| {
            exact_keys(
                proxy,
                &[
                    "allow_local_binding",
                    "allow_upstream_proxy",
                    "credential_broker",
                    "dangerously_allow_all_unix_sockets",
                    "domains",
                    "enable_socks5",
                    "enabled",
                    "mode",
                    "proxy_url",
                    "unix_sockets",
                ],
            )
        })
        .ok_or(DebugProfileError::UnsupportedSettings)?;
    for setting in [
        "allow_local_binding",
        "allow_upstream_proxy",
        "credential_broker",
        "dangerously_allow_all_unix_sockets",
        "enable_socks5",
    ] {
        if proxy.get(setting).and_then(toml::Value::as_bool) != Some(false) {
            return Err(DebugProfileError::UnsupportedSettings);
        }
    }
    if proxy.get("enabled").and_then(toml::Value::as_bool) != Some(true)
        || proxy.get("mode").and_then(toml::Value::as_str) != Some("full")
        || !global_domain_allow(proxy.get("domains"))
        || !loopback_proxy_url(proxy.get("proxy_url"))
        || validate_socket_map(proxy.get("unix_sockets"))? != restricted_socket
    {
        return Err(DebugProfileError::UnsupportedSettings);
    }
    Ok(())
}

fn validate_permission_profile(
    permissions: &toml::Table,
    name: &str,
    parent: &str,
) -> Result<String, DebugProfileError> {
    let profile = permissions
        .get(name)
        .and_then(toml::Value::as_table)
        .filter(|profile| exact_keys(profile, &["extends", "network"]))
        .ok_or(DebugProfileError::UnsupportedSettings)?;
    if profile.get("extends").and_then(toml::Value::as_str) != Some(parent) {
        return Err(DebugProfileError::UnsupportedSettings);
    }
    let network = profile
        .get("network")
        .and_then(toml::Value::as_table)
        .filter(|network| exact_keys(network, &["domains", "enabled", "mode", "unix_sockets"]))
        .ok_or(DebugProfileError::UnsupportedSettings)?;
    if network.get("enabled").and_then(toml::Value::as_bool) != Some(true)
        || network.get("mode").and_then(toml::Value::as_str) != Some("full")
        || !global_domain_allow(network.get("domains"))
    {
        return Err(DebugProfileError::UnsupportedSettings);
    }
    validate_socket_map(network.get("unix_sockets"))
}

fn global_domain_allow(value: Option<&toml::Value>) -> bool {
    value
        .and_then(toml::Value::as_table)
        .is_some_and(|domains| {
            domains.len() == 1 && domains.get("*").and_then(toml::Value::as_str) == Some("allow")
        })
}

fn validate_socket_map(value: Option<&toml::Value>) -> Result<String, DebugProfileError> {
    let sockets = value
        .and_then(toml::Value::as_table)
        .filter(|sockets| sockets.len() == 1)
        .ok_or(DebugProfileError::UnsupportedSettings)?;
    let (path, permission) = sockets
        .iter()
        .next()
        .ok_or(DebugProfileError::UnsupportedSettings)?;
    let path_value = Path::new(path);
    if permission.as_str() != Some("allow")
        || !path_value.is_absolute()
        || path.contains('\0')
        || path_value.file_name().and_then(|name| name.to_str()) != Some("control.sock")
        || path_value
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            != Some("agent-communication")
    {
        return Err(DebugProfileError::UnsupportedSettings);
    }
    Ok(path.to_owned())
}

fn loopback_proxy_url(value: Option<&toml::Value>) -> bool {
    value
        .and_then(toml::Value::as_str)
        .and_then(|url| url.strip_prefix("http://127.0.0.1:"))
        .and_then(|port| port.parse::<u16>().ok())
        .is_some_and(|port| port != 0)
}

fn exact_keys(table: &toml::Table, expected: &[&str]) -> bool {
    table.keys().map(String::as_str).collect::<BTreeSet<_>>()
        == expected.iter().copied().collect::<BTreeSet<_>>()
}
