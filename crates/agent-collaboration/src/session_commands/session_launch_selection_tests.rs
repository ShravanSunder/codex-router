use std::path::Path;
use std::path::PathBuf;

use super::SessionsCommandError;
use super::SessionsLaunchTarget;

fn hosted_target(codex_home: &Path) -> SessionsLaunchTarget {
    SessionsLaunchTarget::Hosted {
        app_server_socket: PathBuf::from("/unused/codex-native.sock"),
        service_directory: PathBuf::from("/unused"),
        codex_home: codex_home.to_path_buf(),
        invoking_cwd: PathBuf::from("/unused"),
        profile: codex_native_integration::SessionProfile::Router,
    }
}

#[test]
fn hosted_resume_names_the_permission_keys_the_router_profile_sets() {
    // Arrange
    let codex_home = tempfile::tempdir().unwrap();
    std::fs::write(
        codex_home.path().join("codex-router.config.toml"),
        "model_provider = \"codex-router\"\nsandbox_mode = \"workspace-write\"\napproval_policy = \"never\"\n",
    )
    .unwrap();

    // Act
    let error = hosted_target(codex_home.path())
        .ensure_profile_allows_remote_resume()
        .unwrap_err();

    // Assert
    assert!(matches!(
        &error,
        SessionsCommandError::ProfileBlocksRemoteResume { keys, .. }
            if keys == "approval_policy, sandbox_mode"
    ));
    assert!(error.to_string().contains("codex-router.config.toml"));
}

#[test]
fn hosted_resume_proceeds_with_a_model_routing_only_profile() {
    // Arrange
    let codex_home = tempfile::tempdir().unwrap();
    std::fs::write(
        codex_home.path().join("codex-router.config.toml"),
        codex_native_integration::CodexRouterProfile::new(8787).render(),
    )
    .unwrap();

    // Act & Assert
    hosted_target(codex_home.path())
        .ensure_profile_allows_remote_resume()
        .unwrap();
}

#[test]
fn local_resume_ignores_profile_permission_keys() {
    // Arrange: a local resume is not remote, so Codex applies the profile's permissions.
    let codex_home = tempfile::tempdir().unwrap();
    std::fs::write(
        codex_home.path().join("codex-router.config.toml"),
        "sandbox_mode = \"workspace-write\"\n",
    )
    .unwrap();
    let target = SessionsLaunchTarget::Local {
        invoking_cwd: codex_home.path().to_path_buf(),
        profile: codex_native_integration::SessionProfile::Router,
    };

    // Act & Assert
    target.ensure_profile_allows_remote_resume().unwrap();
}
