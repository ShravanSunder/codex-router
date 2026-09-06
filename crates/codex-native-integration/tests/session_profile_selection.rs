use std::ffi::OsString;
use std::path::Path;

use codex_native_integration::SessionLaunch;
use codex_native_integration::SessionProfile;

#[test]
fn debug_app_server_profile_preserves_provider_overrides_without_remote_control() {
    let paths = codex_native_integration::CodexPaths::from_codex_home("/Users/owner/.codex".into());
    let profile = codex_native_integration::CodexRouterProfile::new(18787);
    let original = codex_native_integration::AppServerCommandSpec::new(
        &paths,
        &profile,
        Path::new("/tmp/debug-owner/backend.sock"),
    );
    let debug_profile = codex_native_integration::DebugCodexProfile::parse(
        r#"
model = "fixture-model"
model_provider = "codex-router-debug"
[model_providers.codex-router-debug]
name = "Debug provider"
base_url = "http://127.0.0.1:18787/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = true
stream_max_retries = 2
"#,
        18787,
    )
    .unwrap();
    let debug = original.with_debug_profile(&debug_profile);
    assert_eq!(
        debug.environment(),
        vec![(
            "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED".into(),
            "1".into()
        )]
    );
    assert!(
        !debug
            .arguments()
            .iter()
            .any(|argument| argument == "--remote-control")
    );
    assert!(
        debug
            .arguments()
            .iter()
            .any(|argument| argument == "model_provider=\"codex-router-debug\"")
    );
    assert!(
        !debug
            .arguments()
            .iter()
            .any(|argument| argument == "--profile"),
        "direct app-server rejects the client-only profile flag"
    );
    assert_eq!(debug.executable(), paths.managed_executable());
}

#[test]
fn debug_profile_selection_preserves_every_native_launch_tail() {
    // Arrange: the same caller arguments exercise hosted and local launch paths.
    let socket = Path::new("/tmp/debug-owner/backend.sock");
    let cwd = Path::new("/tmp/debug-worktree");
    let arguments = vec![OsString::from("--model"), OsString::from("example-model")];
    let launches = [
        SessionLaunch::new(socket, cwd, &arguments),
        SessionLaunch::resume(socket, cwd, &arguments, "thread-id"),
        SessionLaunch::fork(socket, cwd, &arguments, "thread-id"),
        SessionLaunch::local(cwd, &arguments),
        SessionLaunch::resume_local(cwd, &arguments, "thread-id"),
        SessionLaunch::fork_local(cwd, &arguments, "thread-id"),
    ];
    for launch in launches {
        let original = launch.arguments();
        // Act: profile selection changes only the launcher-owned profile.
        let debug = launch.with_profile(SessionProfile::RouterDebug).arguments();
        // Assert: native arguments, exact target and cwd remain identical.
        let mut expected = original.into_iter();
        assert_eq!(expected.next(), Some(OsString::from("--profile")));
        assert_eq!(expected.next(), Some(OsString::from("codex-router")));
        let expected: Vec<_> = [
            OsString::from("--profile"),
            OsString::from("codex-router-debug"),
        ]
        .into_iter()
        .chain(expected)
        .collect();
        assert_eq!(debug, expected);
    }
}
