use codex_native_integration::{
    AppServerCommandSpec, CodexPaths, CodexRouterProfile, DebugCodexProfile,
    RouterControlSocketPath,
};
use std::path::Path;

const PROFILE: &str = r#"
model = "fixture-model"
model_reasoning_effort = "medium"
model_provider = "codex-router-debug"
[projects."/work/fixture"]
trust_level = "trusted"
[model_providers.codex-router-debug]
name = "Debug provider"
base_url = "http://127.0.0.1:18787/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = true
stream_max_retries = 2
"#;

const NETWORK_PROFILE: &str = r#"
model = "fixture-model"
model_reasoning_effort = "medium"
model_provider = "codex-router-debug"
default_permissions = "router-write-restricted"
[permissions.router-write-restricted]
extends = ":read-only"
[permissions.router-write-restricted.network]
enabled = true
mode = "full"
domains = { "*" = "allow" }
unix_sockets = { "/tmp/debug-router/agent-communication/control.sock" = "allow" }
[permissions.router-workspace-write]
extends = ":workspace"
[permissions.router-workspace-write.network]
enabled = true
mode = "full"
domains = { "*" = "allow" }
unix_sockets = { "/tmp/debug-router/agent-communication/control.sock" = "allow" }
[features.network_proxy]
enabled = true
mode = "full"
proxy_url = "http://127.0.0.1:18788"
enable_socks5 = false
allow_upstream_proxy = false
allow_local_binding = false
credential_broker = false
dangerously_allow_all_unix_sockets = false
domains = { "*" = "allow" }
unix_sockets = { "/tmp/debug-router/agent-communication/control.sock" = "allow" }
[model_providers.codex-router-debug]
name = "Debug provider"
base_url = "http://127.0.0.1:18787/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = true
stream_max_retries = 2
"#;

#[test]
fn backend_overrides_preserve_supported_debug_profile_values() {
    // Arrange.
    let profile = DebugCodexProfile::parse(PROFILE, 18787).unwrap();
    let paths = CodexPaths::from_codex_home("/unused-native-home".into());
    let command = AppServerCommandSpec::new(
        &paths,
        &CodexRouterProfile::new(8787),
        &RouterControlSocketPath::in_collaboration_directory(Path::new(
            "/tmp/debug-proof/agent-communication",
        ))
        .unwrap(),
        Path::new("/tmp/debug-proof/backend.sock"),
    )
    .with_debug_profile(&profile);
    // Act: decode the actual -c values rather than asserting a serializer's spelling.
    let args = command.arguments();
    let mut configuration = String::new();
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        if argument == "-c" {
            configuration.push_str(iter.next().unwrap().to_str().unwrap());
            configuration.push('\n');
        }
    }
    // Assert: no model/project/provider/retry setting was silently dropped.
    assert_eq!(
        toml::from_str::<toml::Table>(&configuration).unwrap(),
        toml::from_str::<toml::Table>(PROFILE).unwrap()
    );
    assert!(
        !args
            .iter()
            .any(|argument| argument == "--profile" || argument == "--remote-control")
    );
    assert_eq!(
        command.environment(),
        vec![(
            "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED".into(),
            "1".into()
        )]
    );
}

#[test]
fn unsafe_or_unrepresentable_profiles_fail_without_exposing_contents() {
    // Arrange / Act / Assert: selection, endpoint and argv-secret boundaries are independent.
    for profile in [
        PROFILE.replace("codex-router-debug", "codex-router"),
        PROFILE.replace("127.0.0.1", "example.invalid"),
        PROFILE.replace("18787", "8787"),
        format!("{PROFILE}\nexperimental_bearer_token = \"fixture-secret\"\n"),
        format!("{PROFILE}\nunknown_setting = true\n"),
    ] {
        let error = DebugCodexProfile::parse(&profile, 18787).err().unwrap();
        assert!(!error.to_string().contains("fixture-secret"));
    }
    assert!(DebugCodexProfile::parse(PROFILE, 8787).is_err());
    assert!(DebugCodexProfile::parse(PROFILE, 18788).is_err());
}

#[test]
fn local_only_profile_settings_are_accepted_but_not_projected() {
    let profile = format!(
        "approval_policy = \"never\"\napprovals_reviewer = \"local\"\nauto_review = \"enabled\"\napps = []\n{PROFILE}"
    );
    let parsed = DebugCodexProfile::parse(&profile, 18787).unwrap();
    let paths = CodexPaths::from_codex_home("/unused-native-home".into());
    let command = AppServerCommandSpec::new(
        &paths,
        &CodexRouterProfile::new(8787),
        &RouterControlSocketPath::in_collaboration_directory(Path::new(
            "/tmp/debug-proof/agent-communication",
        ))
        .unwrap(),
        Path::new("/tmp/debug-proof/backend.sock"),
    )
    .with_debug_profile(&parsed);
    let args = command.arguments();
    for key in [
        "approval_policy",
        "approvals_reviewer",
        "auto_review",
        "apps",
    ] {
        assert!(
            !args
                .iter()
                .any(|argument| argument.to_string_lossy().starts_with(key))
        );
    }
}

#[test]
fn network_experiment_configuration_is_typed_and_closed() {
    assert!(DebugCodexProfile::parse(NETWORK_PROFILE, 18787).is_ok());
    for rejected in [
        NETWORK_PROFILE.replace("127.0.0.1:18788", "0.0.0.0:18788"),
        NETWORK_PROFILE.replace("credential_broker = false", "credential_broker = true"),
        NETWORK_PROFILE.replace(
            "dangerously_allow_all_unix_sockets = false",
            "dangerously_allow_all_unix_sockets = true",
        ),
        NETWORK_PROFILE.replace("mode = \"full\"", "mode = \"limited\""),
        NETWORK_PROFILE.replace("{ \"*\" = \"allow\" }", "{ \"example.com\" = \"allow\" }"),
        NETWORK_PROFILE.replace(
            "control.sock\" = \"allow\"",
            "control.sock\" = \"allow\", \"/tmp/other.sock\" = \"allow\"",
        ),
    ] {
        assert!(DebugCodexProfile::parse(&rejected, 18787).is_err());
    }
}
