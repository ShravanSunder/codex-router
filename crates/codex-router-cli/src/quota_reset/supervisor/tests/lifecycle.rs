use super::*;

#[tokio::test]
async fn precommit_channel_disconnect_reaps_held_gets() {
    let fixture = session_fixture();
    let intents = fixture.ports.intent_sender;
    let session_task = tokio::spawn(fixture.session.run());
    begin_inspection(&intents).await;
    fixture.control.inspection_usage_started.notified().await;
    fixture
        .control
        .inspection_inventory_started
        .notified()
        .await;

    drop(intents);

    assert_eq!(
        session_task.await.expect("session"),
        ResetSessionOutcome::Cancelled
    );
    assert!(
        fixture
            .control
            .inspection_inventory_dropped
            .load(Ordering::SeqCst)
    );
    assert_eq!(fixture.control.post_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn result_can_be_dismissed_before_a_second_inspection() {
    let fixture = session_fixture();
    let mut snapshots = fixture.ports.snapshot_receiver;
    let intents = fixture.ports.intent_sender;
    let session_task = tokio::spawn(fixture.session.run());
    drive_to_committing(&fixture.control, &intents, &mut snapshots).await;
    fixture.control.consume_release.notify_one();
    wait_for_phase(&mut snapshots, WorkflowPhase::Result).await;

    intents
        .send(ResetSessionIntent::DismissResult)
        .await
        .expect("dismiss result");
    wait_for_phase(&mut snapshots, WorkflowPhase::Browse).await;
    begin_inspection(&intents).await;
    fixture.control.revalidation_usage_started.notified().await;
    fixture
        .control
        .revalidation_inventory_started
        .notified()
        .await;
    fixture.control.revalidation_release.notify_waiters();
    wait_for_phase(&mut snapshots, WorkflowPhase::Inspected).await;

    intents
        .send(ResetSessionIntent::Shutdown)
        .await
        .expect("shutdown");
    assert_eq!(
        session_task.await.expect("session"),
        ResetSessionOutcome::Cancelled
    );
    assert_eq!(fixture.control.post_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn fresh_revalidation_facts_replace_cached_inspection_projection() {
    let fixture = session_fixture();
    fixture
        .control
        .revalidation_usage_percent
        .store(1, Ordering::SeqCst);
    let mut snapshots = fixture.ports.snapshot_receiver;
    let intents = fixture.ports.intent_sender;
    let session_task = tokio::spawn(fixture.session.run());
    begin_inspection(&intents).await;
    fixture.control.inspection_usage_started.notified().await;
    fixture
        .control
        .inspection_inventory_started
        .notified()
        .await;
    fixture.control.inspection_usage_release.notify_one();
    fixture.control.inspection_inventory_release.notify_one();
    wait_for_phase(&mut snapshots, WorkflowPhase::Inspected).await;
    intents
        .send(ResetSessionIntent::OpenConfirmation)
        .await
        .expect("open");
    intents
        .send(ResetSessionIntent::SelectYes)
        .await
        .expect("yes");

    intents
        .send(ResetSessionIntent::Confirm {
            now_unix_seconds: 100,
        })
        .await
        .expect("confirm");
    fixture.control.revalidation_usage_started.notified().await;
    fixture
        .control
        .revalidation_inventory_started
        .notified()
        .await;
    fixture.control.revalidation_release.notify_waiters();
    wait_for_weekly_remaining(&mut snapshots, 1).await;

    assert_eq!(snapshots.borrow().phase(), WorkflowPhase::Revalidating);
    assert_eq!(fixture.control.post_calls.load(Ordering::SeqCst), 0);
    intents
        .send(ResetSessionIntent::Shutdown)
        .await
        .expect("shutdown");
    assert_eq!(
        session_task.await.expect("session"),
        ResetSessionOutcome::Cancelled
    );
}

#[tokio::test]
async fn pinned_target_invalidation_cancels_without_retargeting() {
    let fixture = session_fixture();
    let mut snapshots = fixture.ports.snapshot_receiver;
    let intents = fixture.ports.intent_sender;
    let session_task = tokio::spawn(fixture.session.run());
    begin_inspection(&intents).await;
    fixture.control.inspection_usage_started.notified().await;
    fixture
        .control
        .inspection_inventory_started
        .notified()
        .await;

    intents
        .send(ResetSessionIntent::PinnedTargetInvalidated {
            account_id: AccountId::new("acct_supervisor").expect("account id"),
            active_credential_generation: 2,
            reason: PinnedTargetInvalidationReason::AccountRemoved,
        })
        .await
        .expect("invalidate");
    wait_for_phase(&mut snapshots, WorkflowPhase::Browse).await;

    assert_eq!(
        snapshots.borrow().disabled_yes_reason(),
        Some(&ResetEligibilityDisabledReason::PinnedTargetInvalidated(
            PinnedTargetInvalidationReason::AccountRemoved,
        ))
    );
    assert_eq!(
        snapshots.borrow().target().expect("target").account_id,
        AccountId::new("acct_supervisor").expect("account id")
    );
    intents
        .send(ResetSessionIntent::Shutdown)
        .await
        .expect("shutdown");
    assert_eq!(
        session_task.await.expect("session"),
        ResetSessionOutcome::Cancelled
    );
    assert_eq!(fixture.control.post_calls.load(Ordering::SeqCst), 0);
}
