//! Validated in-memory image-auth projection of the saved debug Codex profile.
use codex_native_integration::DebugCodexProfile;
use std::{io::Read as _, path::Path};

pub const REQUIRED_CODEX_VERSION: &str = "0.155.1";

pub fn image_profile(
    codex_home: &Path,
    selected_port: u16,
) -> Result<DebugCodexProfile, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(codex_home.join("codex-router-debug.config.toml"))?;
    let mut source = String::new();
    file.take(65_537).read_to_string(&mut source)?;
    if source.len() > 65_536 {
        return Err("Saved debug profile exceeds the bounded read limit.".into());
    }
    project_image_profile(&source, selected_port)
}

fn project_image_profile(
    source: &str,
    selected_port: u16,
) -> Result<DebugCodexProfile, Box<dyn std::error::Error>> {
    let mut profile: toml::Table = toml::from_str(source)?;
    let provider = profile
        .get_mut("model_providers")
        .and_then(toml::Value::as_table_mut)
        .and_then(|providers| providers.get_mut("codex-router-debug"))
        .and_then(toml::Value::as_table_mut)
        .ok_or("Saved debug profile is missing its sole debug provider.")?;
    let configured_port = provider
        .get("base_url")
        .and_then(toml::Value::as_str)
        .and_then(|value| value.strip_prefix("http://127.0.0.1:"))
        .and_then(|value| value.strip_suffix("/v1"))
        .ok_or("Saved debug profile endpoint is not the loopback v1 route.")?
        .parse::<u16>()?;
    DebugCodexProfile::parse(source, configured_port)?;
    provider.insert(
        "base_url".into(),
        toml::Value::String(format!("http://127.0.0.1:{selected_port}/v1")),
    );
    provider.insert("requires_openai_auth".into(), toml::Value::Boolean(true));
    profile.insert("model".into(), toml::Value::String("gpt-5.6-luna".into()));
    profile.insert(
        "model_reasoning_effort".into(),
        toml::Value::String("high".into()),
    );
    profile
        .entry("features".to_owned())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or("Saved debug profile features setting is not a table.")?
        .insert("image_generation".into(), toml::Value::Boolean(true));
    DebugCodexProfile::parse(&toml::to_string(&profile)?, selected_port).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn feature_only_image_projection_is_admitted() {
        let source = r#"model = "saved-model"
model_reasoning_effort = "low"
model_provider = "codex-router-debug"
[model_providers.codex-router-debug]
name = "Codex Router Debug"
base_url = "http://127.0.0.1:18787/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = true
"#;
        assert!(project_image_profile(source, 28787).is_ok());
        assert!(!source.contains("network_proxy"));
    }
}
