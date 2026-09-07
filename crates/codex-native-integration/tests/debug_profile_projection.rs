use codex_native_integration::{
    AppServerCommandSpec, CodexPaths, CodexRouterProfile, DebugCodexProfile,
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

#[test]
fn backend_overrides_preserve_supported_debug_profile_values() {
    // Arrange.
    let profile = DebugCodexProfile::parse(PROFILE, 18787).unwrap();
    let paths = CodexPaths::from_codex_home("/unused-native-home".into());
    let command = AppServerCommandSpec::new(
        &paths,
        &CodexRouterProfile::new(8787),
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
