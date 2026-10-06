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
        .run_codex_resume(
            &[],
            &crate::sessions::SessionActionSelection::default_catalog(
                SESSION_ID.to_owned(),
                ResumeModelChoice::default(),
            ),
        )
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
        .run_codex_fork(
            &[],
            &crate::sessions::SessionActionSelection::default_catalog(
                SESSION_ID.to_owned(),
                ResumeModelChoice::default(),
            ),
        )
        .unwrap_err();

    // Assert
    assert!(matches!(
        error,
        SessionsCommandError::ProfileBlocksRemoteResume { keys, .. } if keys == ["sandbox_mode"]
    ));
}

#[test]
fn hosted_selection_cannot_fall_back_to_a_local_process_for_resume_or_fork() {
    let selection = crate::sessions::SessionActionSelection {
        identity: crate::sessions::SessionPickerIdentity::HostedCodex(
            serde_json::from_value(serde_json::json!({
                "endpoint": {"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
                "sessionId": SESSION_ID,
            })).unwrap(),
        ),
        source_context: None,
        provenance: crate::sessions::SessionRowProvenance::ObservedHosted,
        model_choice: ResumeModelChoice::default(),
    };
    let mut runner = ProcessSessionsCommandRunner {
        launch_target: SessionsLaunchTarget::Local {
            invoking_cwd: PathBuf::from("/unused"),
            profile: codex_native_integration::SessionProfile::Router,
        },
    };

    for error in [
        runner.run_codex_resume(&[], &selection).unwrap_err(),
        runner.run_codex_fork(&[], &selection).unwrap_err(),
    ] {
        assert!(matches!(
            error,
            SessionsCommandError::SessionSourceUnavailable
        ));
    }
}

#[test]
fn configured_source_context_cannot_be_reinterpreted_as_a_default_native_launch() {
    let registry = crate::sessions::router_connection_registry::RouterConnectionRegistry::parse(
        r#"{
        "version":1,"routers":[{"name":"Configured machine", "connection":{"kind":"remote",
        "serviceId":"00000000-0000-4000-8000-000000000001","mcpUrl":"http://127.0.0.1:18788/mcp"}}]
    }"#,
    )
    .unwrap();
    let mut selection = crate::sessions::SessionActionSelection::default_catalog(
        SESSION_ID.to_owned(),
        ResumeModelChoice::default(),
    );
    selection.source_context = Some(
        crate::presentation::session_picker::PickerSourceContext::ConfiguredHosted(
            registry.routers[0].clone(),
        ),
    );
    let mut runner = ProcessSessionsCommandRunner {
        launch_target: SessionsLaunchTarget::Local {
            invoking_cwd: PathBuf::from("/unused"),
            profile: codex_native_integration::SessionProfile::Router,
        },
    };
    assert!(matches!(
        runner.run_codex_resume(&[], &selection),
        Err(SessionsCommandError::SessionSourceUnqualified)
    ));
    assert!(matches!(
        runner.run_codex_fork(&[], &selection),
        Err(SessionsCommandError::SessionSourceUnqualified)
    ));
}
