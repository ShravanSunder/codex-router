//! Profile keys that make the Codex TUI refuse a remote resume or fork.
//!
//! A remote resume restores the thread's saved permissions, so the TUI treats any
//! permission key in the selected profile as an override it cannot honour and stops
//! with "Permission overrides are not supported when resuming a remote task." It
//! checks presence, not value. Router names those keys before launching instead.

/// Top-level profile keys the Codex TUI treats as a permission override on remote
/// resume and fork (`tui/src/app/config_persistence.rs`,
/// `has_explicit_resume_permission_override`).
pub const REMOTE_RESUME_PERMISSION_KEYS: [&str; 7] = [
    "approval_policy",
    "approvals_reviewer",
    "sandbox_mode",
    "default_permissions",
    "permissions",
    "network",
    "sandbox_workspace_write",
];

/// Returns the permission keys a profile file sets, in
/// [`REMOTE_RESUME_PERMISSION_KEYS`] order.
///
/// # Errors
///
/// Returns the TOML parse error when the profile is not valid TOML.
pub fn profile_remote_resume_permission_keys(
    profile_text: &str,
) -> Result<Vec<&'static str>, toml::de::Error> {
    let profile = profile_text.parse::<toml::Table>()?;
    Ok(REMOTE_RESUME_PERMISSION_KEYS
        .into_iter()
        .filter(|key| profile.contains_key(*key))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::profile_remote_resume_permission_keys;

    #[test]
    fn names_each_permission_key_a_profile_sets() {
        // Arrange
        let profile = r#"
model_provider = "codex-router"
sandbox_mode = "workspace-write"

[permissions.router-workspace-write]
extends = ":workspace"
"#;

        // Act
        let keys = profile_remote_resume_permission_keys(profile).unwrap();

        // Assert
        assert_eq!(keys, ["sandbox_mode", "permissions"]);
    }

    #[test]
    fn model_routing_only_profile_sets_none() {
        // Arrange
        let profile = r#"
model_provider = "codex-router"

[model_providers.codex-router]
base_url = "http://127.0.0.1:8787/v1"
"#;

        // Act
        let keys = profile_remote_resume_permission_keys(profile).unwrap();

        // Assert
        assert!(keys.is_empty());
    }
}
