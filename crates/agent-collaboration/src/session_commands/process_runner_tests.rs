//! The process runner checks the Router profile before it launches Codex.
use std::path::PathBuf;

use super::super::ProcessSessionsCommandRunner;
use super::super::SessionsCommandError;
use super::super::SessionsCommandRunner;
use super::super::SessionsLaunchTarget;
use codex_native_integration::ResumeModelChoice;

const SESSION_ID: &str = "01a0cda2-5931-7c43-90a5-2569c7f0234e";

fn runner_with_blocking_profile(codex_home: &tempfile::TempDir) -> ProcessSessionsCommandRunner {
    std::fs::write(
        codex_home.path().join("codex-router.config.toml"),
        "model_provider = \"codex-router\"\nsandbox_mode = \"workspace-write\"\n",
    )
    .unwrap();
    ProcessSessionsCommandRunner {
        launch_target: SessionsLaunchTarget::Hosted {
            app_server_socket: PathBuf::from("/unused/codex-native.sock"),
            service_directory: codex_home.path().join("missing-service-directory"),
            codex_home: codex_home.path().to_path_buf(),
            invoking_cwd: codex_home.path().to_path_buf(),
            profile: codex_native_integration::SessionProfile::Router,
        },
    }
}

#[test]
fn hosted_resume_stops_before_launch_when_the_profile_sets_a_permission_key() {
    // Arrange
    let codex_home = tempfile::tempdir().unwrap();
    let mut runner = runner_with_blocking_profile(&codex_home);

    // Act
    let error = runner
        .run_codex_resume(&[], SESSION_ID, &ResumeModelChoice::default())
        .unwrap_err();

    // Assert
    assert!(matches!(
        error,
        SessionsCommandError::ProfileBlocksRemoteResume { keys, .. } if keys == ["sandbox_mode"]
    ));
}

#[test]
fn hosted_fork_stops_before_launch_when_the_profile_sets_a_permission_key() {
    // Arrange
    let codex_home = tempfile::tempdir().unwrap();
    let mut runner = runner_with_blocking_profile(&codex_home);

    // Act
    let error = runner
        .run_codex_fork(&[], SESSION_ID, &ResumeModelChoice::default())
        .unwrap_err();

    // Assert
    assert!(matches!(
        error,
        SessionsCommandError::ProfileBlocksRemoteResume { keys, .. } if keys == ["sandbox_mode"]
    ));
}
