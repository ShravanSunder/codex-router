#![cfg(unix)]
#![allow(clippy::expect_used, clippy::panic)]
//! Common Host startup must select the global native CLI, never the bridge SDK bundle.

#[path = "support/global_claude_fixture.rs"]
mod global_claude_fixture;
use collaboration_protocol::EndpointAvailability;
use global_claude_fixture::{GLOBAL_VERSION, HarnessIdentity, HarnessSelectionFixture};

#[tokio::test]
async fn default_global_claude_executes_instead_of_present_sdk_bundle() {
    let fixture = HarnessSelectionFixture::create(true);
    assert!(
        fixture.bundled_executable.is_file(),
        "bundled alternative is present"
    );

    let observed = fixture.observe_startup().await;

    let native_execution = observed
        .native_execution
        .expect("actual nested executable ran");
    assert_eq!(
        native_execution.invoked_executable,
        fixture
            .global_executable
            .canonicalize()
            .expect("fixture global identity"),
        "common startup must execute global Claude even when the adapter has a bundled alternative"
    );
    assert_eq!(native_execution.identity, HarnessIdentity::GlobalHarness);
    assert_eq!(native_execution.version, GLOBAL_VERSION);
    let adapter_dispatch = observed.adapter_dispatch.expect("real adapter dispatched");
    assert_eq!(
        adapter_dispatch.selected_executable,
        native_execution.invoked_executable
    );
    assert_eq!(adapter_dispatch.version_stdout, GLOBAL_VERSION);
    assert!(matches!(
        observed.availability,
        EndpointAvailability::Available { .. }
    ));
}

#[tokio::test]
async fn missing_global_claude_is_actionable_without_adapter_or_bundled_dispatch() {
    let fixture = HarnessSelectionFixture::create(false);
    assert!(
        !fixture.global_executable.exists(),
        "global Claude is absent"
    );
    assert!(
        fixture.bundled_executable.is_file(),
        "bundled alternative is present"
    );

    let observed = fixture.observe_startup().await;

    assert!(
        observed.adapter_dispatch.is_none() && observed.native_execution.is_none(),
        "missing global Claude must stop before adapter dispatch and bundled native execution"
    );
    let EndpointAvailability::Unavailable { reason, fix, .. } = observed.availability else {
        panic!("missing global Claude must publish existing unavailable disposition")
    };
    let reason = String::from(reason).to_lowercase();
    let fix = String::from(fix.expect("actionable global installation guidance")).to_lowercase();
    assert!(reason.contains("global") && reason.contains("claude"));
    assert!(fix.contains("install") && fix.contains("claude"));
    assert!(
        observed.published_channels.is_empty(),
        "the public endpoint must advertise no Claude transport"
    );
}

#[tokio::test]
async fn last_declared_path_binds_global_despite_duplicate_bundled_overrides() {
    let fixture = HarnessSelectionFixture::create(true);
    let observed = fixture
        .observe_startup_with_environment(vec![
            ("PATH".to_owned(), "/missing/intermediate-path".to_owned()),
            (
                "CLAUDE_CODE_EXECUTABLE".to_owned(),
                fixture.bundled_executable.to_string_lossy().into_owned(),
            ),
            (
                "PATH".to_owned(),
                fixture.global_directory.to_string_lossy().into_owned(),
            ),
            (
                "CLAUDE_CODE_EXECUTABLE".to_owned(),
                fixture.bundled_executable.to_string_lossy().into_owned(),
            ),
        ])
        .await;

    let native_execution = observed
        .native_execution
        .expect("actual nested global CLI ran");
    assert_eq!(
        native_execution.invoked_executable,
        fixture
            .global_executable
            .canonicalize()
            .expect("fixture global identity")
    );
    assert_eq!(native_execution.identity, HarnessIdentity::GlobalHarness);
    assert_eq!(native_execution.version, GLOBAL_VERSION);
    let adapter_dispatch = observed.adapter_dispatch.expect("real adapter dispatched");
    assert_eq!(
        adapter_dispatch.selected_executable,
        native_execution.invoked_executable
    );
    assert_eq!(adapter_dispatch.version_stdout, GLOBAL_VERSION);
}
