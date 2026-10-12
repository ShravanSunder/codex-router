use codex_native_integration::{
    AppServerCommandSpec, CodexPaths, CodexRouterProfile, DebugCodexProfile,
    router_permission_profile_overrides,
};
use std::path::Path;

const PROFILE: &str = r#"
model = "fixture-model"
model_reasoning_effort = "medium"
model_provider = "codex-router-debug"
[projects."/work/fixture"]
trust_level = "trusted"
[model_providers.codex-router-debug]
name = "OpenAI"
base_url = "http://127.0.0.1:18787/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = true
stream_max_retries = 2
[features]
enable_request_compression = false
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
[features]
enable_request_compression = false
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
name = "OpenAI"
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
    let mut decoded = toml::Table::new();
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        if argument == "-c" {
            apply_ordered_cli_override(&mut decoded, iter.next().unwrap().to_str().unwrap())
                .unwrap();
        }
    }
    // Assert: no model/project/provider/retry setting was silently dropped, and the
    // debug launch keeps Router's permission profiles so access validation still passes.
    let mut expected = toml::from_str::<toml::Table>(PROFILE).unwrap();
    merge_configuration(
        &mut expected,
        router_permission_profile_overrides()
            .join("\n")
            .parse::<toml::Table>()
            .unwrap(),
    );
    assert_eq!(decoded, expected);
    assert_eq!(
        decoded["model_providers"]["codex-router-debug"]["name"].as_str(),
        Some("OpenAI")
    );
    assert_eq!(
        decoded["features"]["enable_request_compression"].as_bool(),
        Some(false)
    );
    for profile in ["router-write-restricted", "router-workspace-write"] {
        assert_eq!(
            decoded["permissions"][profile]["network"]["enabled"].as_bool(),
            Some(true)
        );
    }
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
fn debug_profile_rejects_current_production_port_and_allows_old_port_as_an_override() {
    let production_profile = PROFILE.replace("18787", "19741");
    assert!(DebugCodexProfile::parse(&production_profile, 19741).is_err());

    let explicit_old_port_profile = PROFILE.replace("18787", "8787");
    assert!(DebugCodexProfile::parse(&explicit_old_port_profile, 8787).is_ok());
}

#[test]
fn local_only_profile_settings_are_accepted_but_not_projected() {
    let profile = format!(
        "approval_policy = \"never\"\napprovals_reviewer = \"local\"\nauto_review = \"enabled\"\napps = []\n{PROFILE}\n[notice.model_migrations]\n\"gpt-5.4-mini\" = \"gpt-6-luna\"\n[tui]\nscreen_reader_detection_done = true\n"
    );
    let parsed = DebugCodexProfile::parse(&profile, 18787).unwrap();
    let paths = CodexPaths::from_codex_home("/unused-native-home".into());
    let command = AppServerCommandSpec::new(
        &paths,
        &CodexRouterProfile::new(8787),
        Path::new("/tmp/debug-proof/backend.sock"),
    )
    .with_debug_profile(&parsed);
    let args = command.arguments();
    for key in [
        "approval_policy",
        "approvals_reviewer",
        "auto_review",
        "apps",
        "notice",
        "tui",
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

#[test]
fn image_generation_profile_accepts_openai_auth_without_weakening_network_validation() {
    let image_profile = format!("{PROFILE}\nimage_generation = true\n").replace(
        "requires_openai_auth = false",
        "requires_openai_auth = true",
    );
    let image_network_profile = NETWORK_PROFILE
        .replace(
            "[features.network_proxy]",
            "image_generation = true\n[features.network_proxy]",
        )
        .replace(
            "requires_openai_auth = false",
            "requires_openai_auth = true",
        );

    assert!(DebugCodexProfile::parse(&image_profile, 18787).is_ok());
    assert!(DebugCodexProfile::parse(&image_network_profile, 18787).is_ok());
    for rejected in [
        image_profile.replace("image_generation = true", "image_generation = \"true\""),
        image_profile.replace(
            "image_generation = true",
            "image_generation = true\nunknown_feature = true",
        ),
        image_network_profile.replace("credential_broker = false", "credential_broker = true"),
    ] {
        assert!(DebugCodexProfile::parse(&rejected, 18787).is_err());
    }
}

#[test]
fn debug_profile_requires_literal_native_capability_and_uncompressed_requests() {
    for valid in [PROFILE, NETWORK_PROFILE] {
        for invalid in [
            valid.replace("name = \"OpenAI\"", "name = \"codex-router-debug\""),
            valid.replace("name = \"OpenAI\"", "name = \"openai\""),
            valid.replace(
                "enable_request_compression = false",
                "enable_request_compression = true",
            ),
            valid.replace(
                "enable_request_compression = false",
                "enable_request_compression = \"false\"",
            ),
            valid.replace("enable_request_compression = false\n", ""),
        ] {
            assert!(
                DebugCodexProfile::parse(&invalid, 18787).is_err(),
                "invalid native capability accepted"
            );
        }
    }
}

// Merge the completed CLI layer onto normal-home configuration, not each -c value.
fn merge_configuration(configuration: &mut toml::Table, overrides: toml::Table) {
    for (key, value) in overrides {
        match (configuration.get_mut(&key), value) {
            (Some(toml::Value::Table(existing)), toml::Value::Table(next)) => {
                merge_configuration(existing, next);
            }
            (_, replacement) => {
                configuration.insert(key, replacement);
            }
        }
    }
}

// Native builds ONE CLI layer in argv order, replacing the value at the final path.
// These emitted paths are unquoted; root table assignments replace earlier subtrees.
fn apply_ordered_cli_override(
    configuration: &mut toml::Table,
    argument: &str,
) -> Result<(), &'static str> {
    let (path, literal) = argument
        .split_once('=')
        .ok_or("missing override assignment")?;
    let mut parsed = format!("value={literal}")
        .parse::<toml::Table>()
        .map_err(|_| "invalid override value")?;
    let value = parsed.remove("value").ok_or("missing override value")?;
    let segments: Vec<_> = path.split('.').collect();
    let (leaf, parents) = segments.split_last().ok_or("missing override path")?;
    let mut current = configuration;
    for parent in parents {
        current = current
            .entry((*parent).to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or("override parent is not a table")?;
    }
    if path == "features.network_proxy"
        && let Some(toml::Value::Table(existing)) = current.get_mut(*leaf)
        && let toml::Value::Table(next) = &value
    {
        // This two-segment structured feature merges at its leaf in pinned Codex.
        merge_configuration(existing, next.clone());
    } else {
        current.insert((*leaf).to_owned(), value);
    }
    Ok(())
}

#[test]
fn minimal_debug_features_preserve_network_disable_in_ordered_cli_layer() {
    let profile = DebugCodexProfile::parse(PROFILE, 18787).unwrap();
    let paths = CodexPaths::from_codex_home("/unused-native-home".into());
    let command = AppServerCommandSpec::new(
        &paths,
        &CodexRouterProfile::new(8787),
        Path::new("/tmp/debug-proof/backend.sock"),
    )
    .with_debug_profile(&profile);
    let args = command.arguments();
    let overrides: Vec<_> = args
        .windows(2)
        .filter(|pair| pair[0] == "-c")
        .map(|pair| pair[1].to_str().unwrap())
        .collect();
    let mut cli_layer = toml::Table::new();
    for argument in &overrides {
        apply_ordered_cli_override(&mut cli_layer, argument).unwrap();
    }
    assert_eq!(
        cli_layer["features"]
            .get("network_proxy")
            .and_then(|proxy| proxy.get("enabled"))
            .and_then(toml::Value::as_bool),
        Some(false),
        "later compression projection must not erase the earlier CLI network disable"
    );
    assert!(overrides.contains(&"features.network_proxy.enabled=false"));
    assert!(overrides.contains(&"features.enable_request_compression=false"));
    assert!(
        !overrides
            .iter()
            .any(|argument| argument.starts_with("features="))
    );
    let mut home = "[features.network_proxy]\nenabled=true\nproxy_url=\"http://127.0.0.1:9999\"\n"
        .parse::<toml::Table>()
        .unwrap();
    merge_configuration(&mut home, cli_layer);
    assert_eq!(
        home["features"]["network_proxy"]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(
        home["features"]["enable_request_compression"].as_bool(),
        Some(false)
    );
}

#[test]
fn explicit_debug_network_and_image_features_keep_validated_override_precedence() {
    for text in [
        NETWORK_PROFILE.to_owned(),
        NETWORK_PROFILE
            .replace(
                "[features.network_proxy]",
                "image_generation = true\n[features.network_proxy]",
            )
            .replace(
                "requires_openai_auth = false",
                "requires_openai_auth = true",
            ),
    ] {
        let profile = DebugCodexProfile::parse(&text, 18787).unwrap();
        let paths = CodexPaths::from_codex_home("/unused-native-home".into());
        let command = AppServerCommandSpec::new(
            &paths,
            &CodexRouterProfile::new(8787),
            Path::new("/tmp/debug-proof/backend.sock"),
        )
        .with_debug_profile(&profile);
        let args = command.arguments();
        let mut actual = toml::Table::new();
        let overrides: Vec<_> = args
            .windows(2)
            .filter(|pair| pair[0] == "-c")
            .map(|pair| pair[1].to_str().unwrap())
            .collect();
        for argument in &overrides {
            apply_ordered_cli_override(&mut actual, argument).unwrap();
        }
        let expected = text.parse::<toml::Table>().unwrap();
        assert_eq!(actual["features"], expected["features"]);
        assert_eq!(actual["permissions"], expected["permissions"]);
        assert_eq!(actual["model_providers"], expected["model_providers"]);
        assert_eq!(actual["model"], expected["model"]);
        assert_eq!(
            actual["model_reasoning_effort"],
            expected["model_reasoning_effort"]
        );
        assert_eq!(
            actual["default_permissions"],
            expected["default_permissions"]
        );
        assert_eq!(
            actual["features"]["network_proxy"]["enabled"].as_bool(),
            Some(true)
        );
        assert!(
            !overrides
                .iter()
                .any(|argument| argument.starts_with("features="))
        );
        if text.contains("image_generation = true") {
            assert!(overrides.contains(&"features.image_generation=true"));
        }
    }
}
