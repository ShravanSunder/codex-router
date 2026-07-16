use super::*;

#[test]
fn non_spawning_service_and_intent_first_supervisor_keep_effect_ownership() {
    let service_source = include_str!("../../service.rs");
    let owns_spawn_or_join_handle = service_source.contains("tokio::spawn")
        || service_source.contains("JoinHandle")
        || service_source.contains("JoinSet");
    assert!(!owns_spawn_or_join_handle);

    let supervisor_source = include_str!("../../supervisor.rs");
    for forbidden_dependency in ["sqlx::", "StateStore", "persist_", "refresh_quota"] {
        assert!(!supervisor_source.contains(forbidden_dependency));
    }
    let select_source = supervisor_source
        .split_once("tokio::select!")
        .and_then(|(_before, select_and_tail)| {
            select_and_tail
                .split_once("async fn apply_intent")
                .map(|(select_source, _after)| select_source)
        })
        .unwrap_or_default();
    assert!(
        !select_source.is_empty(),
        "session select source was absent"
    );
    assert!(select_source.contains("biased;"));
    assert!(
        select_source.find("intent =").expect("intent branch")
            < select_source
                .find("completed = self.tasks")
                .expect("task branch"),
        "queued precommit intents must be polled before ready effect completions"
    );
}

#[tokio::test]
async fn cancel_drains_a_started_authority_read_before_session_cleanup() {
    let authority_started = Arc::new(Notify::new());
    let authority_release = Arc::new(Notify::new());
    let provider_control = Arc::new(FakeProviderControl::default());
    let service = ResetWorkflowService::new(
        HeldAuthorityReader {
            started: Arc::clone(&authority_started),
            release: Arc::clone(&authority_release),
        },
        FakeProvider {
            control: provider_control,
        },
    );
    let (session, ports) = QuotaInteractiveSession::new(
        service,
        FakeRedeemRequestIdFactory {
            mints: Arc::new(AtomicUsize::new(0)),
        },
        Arc::new(FakeResetClock),
    );
    let mut snapshots = ports.snapshot_receiver;
    let intents = ports.intent_sender;
    let session_task = tokio::spawn(session.run());
    begin_inspection(&intents).await;
    authority_started.notified().await;

    intents
        .send(ResetSessionIntent::Cancel)
        .await
        .expect("cancel intent");
    tokio::task::yield_now().await;

    assert!(!session_task.is_finished());
    authority_release.notify_one();
    wait_for_phase(&mut snapshots, WorkflowPhase::Browse).await;
    intents
        .send(ResetSessionIntent::Shutdown)
        .await
        .expect("shutdown intent");
    assert_eq!(
        session_task.await.expect("session task"),
        ResetSessionOutcome::Cancelled
    );
}

#[tokio::test]
async fn credit_expiring_at_commit_time_refuses_before_redeem_id_mint_or_post() {
    let control = Arc::new(FakeProviderControl::default());
    let mints = Arc::new(AtomicUsize::new(0));
    let service = ResetWorkflowService::new(
        FakeAuthorityReader {
            reads: Arc::new(AtomicUsize::new(0)),
        },
        FakeProvider {
            control: Arc::clone(&control),
        },
    );
    let (session, ports) = QuotaInteractiveSession::new(
        service,
        FakeRedeemRequestIdFactory {
            mints: Arc::clone(&mints),
        },
        Arc::new(ScriptedResetClock {
            times: Mutex::new(VecDeque::from([100, 200])),
        }),
    );
    let mut snapshots = ports.snapshot_receiver;
    let intents = ports.intent_sender;
    let session_task = tokio::spawn(session.run());
    begin_inspection(&intents).await;
    control.inspection_usage_started.notified().await;
    control.inspection_inventory_started.notified().await;
    control.inspection_usage_release.notify_one();
    control.inspection_inventory_release.notify_one();
    wait_for_phase(&mut snapshots, WorkflowPhase::Inspected).await;
    intents
        .send(ResetSessionIntent::OpenConfirmation)
        .await
        .expect("open confirmation");
    intents
        .send(ResetSessionIntent::SelectYes)
        .await
        .expect("select yes");

    intents
        .send(ResetSessionIntent::Confirm {
            now_unix_seconds: 100,
        })
        .await
        .expect("confirm");
    control.revalidation_usage_started.notified().await;
    control.revalidation_inventory_started.notified().await;
    control.revalidation_release.notify_waiters();
    wait_for_phase(&mut snapshots, WorkflowPhase::Result).await;

    assert_eq!(
        snapshots.borrow().result(),
        Some(&WorkflowResult::Refused(
            RenderSafeFailure::SelectedCreditChanged
        ))
    );
    assert_eq!(mints.load(Ordering::SeqCst), 0);
    assert_eq!(control.post_calls.load(Ordering::SeqCst), 0);
    intents
        .send(ResetSessionIntent::Shutdown)
        .await
        .expect("shutdown");
    assert_eq!(
        session_task.await.expect("session task"),
        ResetSessionOutcome::Cancelled
    );
}

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
async fn ineligible_revalidation_enters_a_dismissible_refusal_result() {
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

    assert_eq!(snapshots.borrow().phase(), WorkflowPhase::Result);
    assert_eq!(
        snapshots.borrow().result(),
        Some(&WorkflowResult::Refused(
            RenderSafeFailure::EligibilityRefused
        ))
    );
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
