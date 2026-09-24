//! Merge owner-edited provider defaults with explicit Host flags for this start.
use super::HostCommandError;
use codex_router_host::{
    ExternalProviderLaunchBinding, ExternalProviderStartup, ProviderConfigurationEntry,
    ProviderConfigurationFile,
};
use collaboration_protocol::ProviderKind;
use std::path::{Path, PathBuf};

pub(super) fn configured_provider_startups(
    router_root: &Path,
    explicit_launches: &[ExternalProviderLaunchBinding],
) -> Result<Vec<ExternalProviderStartup>, HostCommandError> {
    let configuration = ProviderConfigurationFile::load_or_create(router_root);
    match configuration {
        Ok(configuration) => Ok(vec![
            configured_provider(
                ProviderKind::ClaudeCode,
                configuration.claude(),
                explicit_launches,
            )?,
            configured_provider(
                ProviderKind::Cursor,
                configuration.cursor(),
                explicit_launches,
            )?,
        ]),
        Err(error) => {
            let reason = error.to_string();
            let fix = format!(
                "correct {} and restart the Host",
                router_root.join("providers.json").display()
            );
            Ok(vec![
                unavailable(ProviderKind::ClaudeCode, reason.clone(), fix.clone())?,
                unavailable(ProviderKind::Cursor, reason, fix)?,
            ])
        }
    }
}

fn configured_provider(
    provider: ProviderKind,
    entry: &ProviderConfigurationEntry,
    explicit_launches: &[ExternalProviderLaunchBinding],
) -> Result<ExternalProviderStartup, HostCommandError> {
    if let Some(explicit) = explicit_launches
        .iter()
        .find(|binding| binding.provider() == provider)
    {
        return Ok(ExternalProviderStartup::Launch(explicit.clone()));
    }
    if !entry.enabled {
        return unavailable(
            provider,
            "disabled in providers.json".to_owned(),
            "enable this provider in providers.json at the router root and restart the Host"
                .to_owned(),
        );
    }
    let executable = entry.executable.clone().unwrap_or_else(|| {
        PathBuf::from(match provider {
            ProviderKind::ClaudeCode => "claude-agent-acp",
            ProviderKind::Cursor => "agent",
        })
    });
    let binding = match provider {
        ProviderKind::ClaudeCode => {
            ExternalProviderLaunchBinding::claude(executable, entry.arguments.clone())
        }
        ProviderKind::Cursor => {
            ExternalProviderLaunchBinding::cursor(executable, entry.arguments.clone())
        }
    }
    .map_err(|error| HostCommandError::RouterRoot(error.to_owned()))?;
    Ok(ExternalProviderStartup::Launch(binding))
}

fn unavailable(
    provider: ProviderKind,
    reason: String,
    fix: String,
) -> Result<ExternalProviderStartup, HostCommandError> {
    ExternalProviderStartup::unavailable(provider, reason, fix)
        .map_err(|error| HostCommandError::RouterRoot(error.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_selects_both_providers_without_flags() {
        let root = tempfile::tempdir().expect("router root");

        let startups = configured_provider_startups(root.path(), &[]).expect("defaults");

        assert_eq!(startups.len(), 2);
        let ExternalProviderStartup::Launch(claude) = &startups[0] else {
            panic!("Claude should launch")
        };
        assert_eq!(claude.launch.executable, PathBuf::from("claude-agent-acp"));
        let ExternalProviderStartup::Launch(cursor) = &startups[1] else {
            panic!("Cursor should launch")
        };
        assert_eq!(cursor.launch.executable, PathBuf::from("agent"));
        assert_eq!(cursor.launch.arguments, vec!["acp"]);
        assert!(root.path().join("providers.json").exists());
    }

    #[test]
    fn flag_overrides_disabled_entry_for_one_start_only() {
        let root = tempfile::tempdir().expect("router root");
        let path = root.path().join("providers.json");
        let bytes = br#"{"version":1,"providers":{"claude":{"enabled":false,"executable":null,"arguments":[]},"cursor":{"enabled":true,"executable":"/configured/agent","arguments":["acp"]}}}"#;
        std::fs::write(&path, bytes).expect("configuration");
        let explicit = ExternalProviderLaunchBinding::claude(
            PathBuf::from("/override/claude-agent-acp"),
            vec!["--flag".to_owned()],
        )
        .expect("explicit launch");

        let startups = configured_provider_startups(root.path(), &[explicit]).expect("merged");

        let ExternalProviderStartup::Launch(claude) = &startups[0] else {
            panic!("explicit Claude should launch")
        };
        assert_eq!(
            claude.launch.executable,
            PathBuf::from("/override/claude-agent-acp")
        );
        assert_eq!(claude.launch.arguments, vec!["--flag"]);
        let ExternalProviderStartup::Launch(cursor) = &startups[1] else {
            panic!("configured Cursor should launch")
        };
        assert_eq!(cursor.launch.executable, PathBuf::from("/configured/agent"));
        assert_eq!(std::fs::read(&path).expect("unchanged file"), bytes);
    }

    #[test]
    fn malformed_file_marks_both_unavailable_without_replacement() {
        let root = tempfile::tempdir().expect("router root");
        let path = root.path().join("providers.json");
        let bytes = b"{not-json";
        std::fs::write(&path, bytes).expect("malformed file");

        let startups = configured_provider_startups(root.path(), &[]).expect("isolated failure");

        for startup in startups {
            let ExternalProviderStartup::Unavailable { reason, fix, .. } = startup else {
                panic!("malformed file cannot launch a provider")
            };
            assert!(String::from(reason).contains(&path.display().to_string()));
            assert!(String::from(fix).contains("correct"));
        }
        assert_eq!(std::fs::read(&path).expect("preserved file"), bytes);
    }
}
