use codex_router_keeper::{GroupLaunchOutcome, GroupStopError, OwnedProcessGroup};
#[tokio::test]
async fn pre_spawn_refusal_has_no_child_or_cleanup_debt() {
    let command = tokio::process::Command::new("/nonexistent/owned-launch-failure-fixture");
    assert!(matches!(
        OwnedProcessGroup::spawn(command).await,
        GroupLaunchOutcome::Refused {
            reason: GroupStopError::Spawn(_)
        }
    ));
}
