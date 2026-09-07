//! Invocation-local hook isolation; never edits user hooks, profiles or environment.
use serde_json::{Value, json};
use tokio::process::Command;

pub fn configure_owned_host(command: &mut Command) {
    // The installed personal Stop hook supports this runner override. Its usual
    // runner launches a classifier via port8787. A harmless successful no-op
    // leaves no classifier output, so that hook fails open without model traffic.
    // This also covers a native/ACP cold resume which rebuilds thread config.
    command.env("CODEX_STOP_REVIEW_RUNNER", "/usr/bin/true");
}

pub fn thread_overrides(configuration: Option<Value>) -> Result<Value, &'static str> {
    let mut configuration = configuration.unwrap_or_else(|| json!({}));
    configuration
        .as_object_mut()
        .ok_or("proof configuration must be an object")?
        .insert("features.hooks".to_owned(), json!(false));
    Ok(configuration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_isolation_is_scoped_to_owned_command_and_preserves_native_overrides() {
        // Arrange: existing command/config settings and the unchanged parent environment.
        let inherited = std::env::var_os("CODEX_STOP_REVIEW_RUNNER");
        let mut command = Command::new("unused-proof-executable");
        command.env("UNCHANGED_PROOF_SETTING", "retained");
        let configuration = json!({"features.network_proxy":{"enabled":true},"permissions":{"proof":{"extends":":read-only"}}});
        // Act: configure only this child invocation and this thread request.
        configure_owned_host(&mut command);
        let overrides = thread_overrides(Some(configuration.clone())).unwrap();
        // Assert: no global change or broad configuration replacement.
        assert_eq!(std::env::var_os("CODEX_STOP_REVIEW_RUNNER"), inherited);
        assert!(
            command
                .as_std()
                .get_envs()
                .any(|(key, value)| key == "CODEX_STOP_REVIEW_RUNNER"
                    && value == Some(std::ffi::OsStr::new("/usr/bin/true")))
        );
        assert!(
            command
                .as_std()
                .get_envs()
                .any(|(key, value)| key == "UNCHANGED_PROOF_SETTING"
                    && value == Some(std::ffi::OsStr::new("retained")))
        );
        assert_eq!(overrides.get("features.hooks"), Some(&json!(false)));
        assert_eq!(
            overrides.get("features.network_proxy"),
            configuration.get("features.network_proxy")
        );
        assert_eq!(
            overrides.get("permissions"),
            configuration.get("permissions")
        );
        assert!(thread_overrides(Some(json!([]))).is_err());
    }
}
