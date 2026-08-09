use std::ffi::OsString;
use std::path::PathBuf;

use codex_router_codex::AppServerCommandSpec;
use codex_router_codex::CodexPaths;
use codex_router_codex::CodexRouterProfile;
use codex_router_codex::SessionLaunch;

#[test]
fn codex_paths_keep_native_state_under_normal_codex_home() {
    let paths = CodexPaths::from_codex_home(PathBuf::from("/Users/owner/.codex"));

    assert_eq!(
        paths.app_server_socket(),
        PathBuf::from("/Users/owner/.codex/app-server-control/app-server-control.sock")
    );
    assert_eq!(
        paths.managed_executable(),
        PathBuf::from("/Users/owner/.codex/packages/standalone/current/codex")
    );
}

#[test]
fn router_profile_has_one_rendering_and_root_override_projection() {
    let profile = CodexRouterProfile::new(8787);

    assert_eq!(
        profile.render(),
        concat!(
            "model_provider = \"codex-router\"\n\n",
            "[model_providers.codex-router]\n",
            "name = \"codex-router\"\n",
            "base_url = \"http://127.0.0.1:8787/v1\"\n",
            "wire_api = \"responses\"\n",
            "requires_openai_auth = false\n",
            "supports_websockets = true\n",
        )
    );
    assert_eq!(
        profile.root_overrides(),
        vec![
            "model_provider=\"codex-router\"".to_owned(),
            "model_providers.codex-router.name=\"codex-router\"".to_owned(),
            "model_providers.codex-router.base_url=\"http://127.0.0.1:8787/v1\"".to_owned(),
            "model_providers.codex-router.wire_api=\"responses\"".to_owned(),
            "model_providers.codex-router.requires_openai_auth=false".to_owned(),
            "model_providers.codex-router.supports_websockets=true".to_owned(),
        ]
    );
}

#[test]
fn app_server_command_uses_managed_executable_profile_and_native_contract() {
    let paths = CodexPaths::from_codex_home(PathBuf::from("/Users/owner/.codex"));
    let socket = paths.app_server_socket();
    let command = AppServerCommandSpec::new(&paths, &CodexRouterProfile::new(8787), &socket);

    assert_eq!(command.executable(), paths.managed_executable());
    assert_eq!(
        command.arguments(),
        vec![
            OsString::from("-c"),
            OsString::from("model_provider=\"codex-router\""),
            OsString::from("-c"),
            OsString::from("model_providers.codex-router.name=\"codex-router\""),
            OsString::from("-c"),
            OsString::from("model_providers.codex-router.base_url=\"http://127.0.0.1:8787/v1\"",),
            OsString::from("-c"),
            OsString::from("model_providers.codex-router.wire_api=\"responses\""),
            OsString::from("-c"),
            OsString::from("model_providers.codex-router.requires_openai_auth=false"),
            OsString::from("-c"),
            OsString::from("model_providers.codex-router.supports_websockets=true"),
            OsString::from("app-server"),
            OsString::from("--remote-control"),
            OsString::from("--listen"),
            OsString::from("unix:///Users/owner/.codex/app-server-control/app-server-control.sock",),
        ]
    );
}

#[test]
fn session_launch_keeps_remote_at_root_for_new_and_resume() {
    let socket = PathBuf::from("/Users/owner/.codex/app-server-control/app-server-control.sock");
    let user_arguments = vec![OsString::from("--model"), OsString::from("gpt-5.4")];

    assert_eq!(
        SessionLaunch::new(&socket, &user_arguments).arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--remote"),
            OsString::from("unix:///Users/owner/.codex/app-server-control/app-server-control.sock"),
            OsString::from("--model"),
            OsString::from("gpt-5.4"),
        ]
    );
    assert_eq!(
        SessionLaunch::resume(&socket, &user_arguments, "thread_123").arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--remote"),
            OsString::from("unix:///Users/owner/.codex/app-server-control/app-server-control.sock"),
            OsString::from("--model"),
            OsString::from("gpt-5.4"),
            OsString::from("resume"),
            OsString::from("--"),
            OsString::from("thread_123"),
        ]
    );
}

#[test]
fn local_session_launch_keeps_router_profile_without_remote_attachment() {
    let user_arguments = vec![
        OsString::from("--model"),
        OsString::from("gpt-5.6-luna"),
        OsString::from("--yolo"),
    ];

    assert_eq!(
        SessionLaunch::local(&user_arguments).arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--model"),
            OsString::from("gpt-5.6-luna"),
            OsString::from("--yolo"),
        ]
    );
    assert_eq!(
        SessionLaunch::resume_local(&user_arguments, "thread_123").arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--model"),
            OsString::from("gpt-5.6-luna"),
            OsString::from("--yolo"),
            OsString::from("resume"),
            OsString::from("--"),
            OsString::from("thread_123"),
        ]
    );
}
