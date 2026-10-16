//! L3 normal-prefix and unavailable-socket cases through the real local Runtime and SQLite.
use crate::{
    owned_resource_cleanup::{
        CleanupAction, CleanupObservation, CleanupOutcome, MatrixCleanupReport,
        cleanup_owned_resources,
    },
    owned_resources::InstructionCreation,
    proof_context::ProofResult,
};
use automation_storage::AutomationStore;
use collaboration_client::{
    CollaborationClient,
    board::{EndReason, SubscriptionScope, SubscriptionState, ThreadShowRequest},
    protocol::{
        EndpointRef, SavedMessage, ScheduleShowRequest, SessionRef, ThreadSubscriptionState,
        ThreadSubscriptionsRequest, WakeShowRequest, WakeState,
    },
};
use message_board_storage::BoardStore;
use std::{fmt, path::Path};

#[path = "owned_resource_cleanup_producer_report.rs"]
mod producer_report;
use producer_report::{ActualHelperPrefix, check_actual_helper_report, missing_installed_pairs};

#[path = "owned_resource_cleanup_runtime_fixture.rs"]
mod runtime_fixture;
use runtime_fixture::{
    ActorRootExpectation, ExpectedResources, FixtureCleanupIssue, RuntimeCleanupFixture,
    RuntimePrefix, ensure,
};

#[tokio::test]
async fn runtime_error_after_wake_receipt_retires_known_prefix() -> ProofResult<()> {
    run_case(RuntimePrefix::WakeReceipt, [1, 0, 4, 4]).await
}
#[tokio::test]
async fn runtime_schedule_before_prepare_is_cleanup_owned() -> ProofResult<()> {
    run_case(RuntimePrefix::BeforePrepare, [1, 1, 4, 4]).await
}
#[tokio::test]
async fn runtime_enabled_peer_schedule_becomes_disabled() -> ProofResult<()> {
    run_case(RuntimePrefix::EnabledSchedule, [1, 1, 4, 4]).await
}
#[tokio::test]
async fn runtime_first_reader_is_retired_before_second_root_exists() -> ProofResult<()> {
    run_case(RuntimePrefix::FirstReader, [0, 0, 2, 2]).await
}
#[tokio::test]
async fn stopped_runtime_keeps_all_cleanup_attempts_unknown_without_retry() -> ProofResult<()> {
    run_case(RuntimePrefix::UnavailableApi, [1, 1, 4, 4]).await
}

async fn run_case(prefix: RuntimePrefix, expected_counts: [usize; 4]) -> ProofResult<()> {
    let directory = tempfile::Builder::new()
        .prefix("u6-l3-")
        .tempdir_in("/tmp")?;
    let mut fixture = RuntimeCleanupFixture::allocate(directory.path())?;
    let scenario: ProofResult<()> =
        async {
            fixture.start().await?;
            fixture.build_prefix(prefix).await?;
            check_fixture_stores(&mut fixture, StoreExpectation::Unchanged(prefix)).await?;
            match prefix {
                RuntimePrefix::FirstReader
                | RuntimePrefix::WakeReceipt
                | RuntimePrefix::BeforePrepare => Err(Box::new(InjectedPrefixFailure(prefix))
                    as Box<dyn std::error::Error + Send + Sync>),
                RuntimePrefix::EnabledSchedule | RuntimePrefix::UnavailableApi => Ok(()),
            }
        }
        .await;
    // Preserve the scenario result until every cleanup, teardown and reopen operation finishes.
    let stopped_api = if prefix == RuntimePrefix::UnavailableApi {
        fixture.stop_runtime().await
    } else {
        Ok(())
    };
    let expected = fixture.expected.clone();
    let store_expectation = if prefix == RuntimePrefix::UnavailableApi {
        StoreExpectation::Unchanged(prefix)
    } else {
        StoreExpectation::Retired
    };
    let cleanup_observation: ProofResult<()> = async {
        let owned = fixture
            .owned
            .take()
            .ok_or("no actual client/owner available for cleanup")?;
        let report =
            cleanup_owned_resources(&owned.client, owned.resources.into_cleanup_plan()).await;
        check_report(
            &report,
            &expected,
            expected_counts,
            prefix == RuntimePrefix::UnavailableApi,
        )?;
        if prefix != RuntimePrefix::UnavailableApi {
            check_public_state(&owned.client, &expected).await?;
        }
        check_fixture_stores(&mut fixture, store_expectation).await
    }
    .await;
    let teardown = fixture.finish().await;
    let reopen = check_reopened_stores(directory.path(), &expected, store_expectation).await;
    let cleanup_succeeded =
        stopped_api.is_ok() && cleanup_observation.is_ok() && teardown.is_empty() && reopen.is_ok();
    let scenario = assess_scenario(prefix, scenario, cleanup_succeeded);
    match (scenario, stopped_api, cleanup_observation, teardown, reopen) {
        (Ok(()), Ok(()), Ok(()), teardown, Ok(())) if teardown.is_empty() => Ok(()),
        (scenario, stopped_api, cleanup_observation, teardown, reopen) => {
            Err(Box::new(RuntimeCaseFailure {
                scenario: scenario.err(),
                stopped_api: stopped_api.err(),
                cleanup_observation: cleanup_observation.err(),
                teardown,
                reopen: reopen.err(),
            }))
        }
    }
}

fn check_report(
    report: &MatrixCleanupReport,
    expected: &ExpectedResources,
    expected_counts: [usize; 4],
    unavailable: bool,
) -> ProofResult<()> {
    let mut counts = [0_usize; 4];
    let [wake_count, schedule_count, unsubscribe_count, unwatch_count] = &mut counts;
    let mut cleanup_operations = Vec::new();
    ensure(
        report.unresolved.is_empty(),
        "known Runtime receipts became unresolved creations",
    )?;
    ensure(
        report.history.actor_roots.len() == expected.pairs.len(),
        "cleanup lost actor/root history",
    )?;
    for pair in &expected.pairs {
        ensure(
            report.history.actor_roots.iter().any(|actual| {
                actual.actor == pair.actor && actual.root_message_id == pair.root_message_id
            }),
            "cleanup changed actual actor/root history",
        )?;
    }
    if let Some(instruction_id) = &expected.instruction {
        ensure(
            matches!(&report.history.instruction, Some(InstructionCreation::Known { instruction_id: actual, .. }) if actual == instruction_id),
            "cleanup discarded actual instruction history",
        )?;
    }
    for attempt in &report.attempts {
        match &attempt.action {
            CleanupAction::CancelWake {
                operation_id,
                wakeup_id,
            } => {
                *wake_count += 1;
                let (creation_operation, actual_id) =
                    expected.wake.as_ref().ok_or("unexpected wake cleanup")?;
                ensure(
                    wakeup_id == actual_id && operation_id != creation_operation,
                    "wake cleanup changed ownership or reused create operation",
                )?;
                cleanup_operations.push(operation_id);
                if !unavailable {
                    ensure(
                        matches!(&attempt.outcome, CleanupOutcome::Returned(CleanupObservation::Wake {
                        state: WakeState::Cancelled, next_due_at: None, pending_delivery_id: None,
                        discarded_delivery_ids, retained_deliveries,
                    }) if discarded_delivery_ids.is_empty() && retained_deliveries.is_empty()),
                        "real wake was not locally Cancelled",
                    )?;
                }
            }
            CleanupAction::DisableSchedule {
                operation_id,
                schedule_id,
            } => {
                *schedule_count += 1;
                let (creation_operation, actual_id) = expected
                    .schedule
                    .as_ref()
                    .ok_or("unexpected schedule cleanup")?;
                ensure(
                    schedule_id == actual_id && operation_id != creation_operation,
                    "schedule cleanup changed ownership or reused create operation",
                )?;
                cleanup_operations.push(operation_id);
                if !unavailable {
                    ensure(
                        matches!(
                            &attempt.outcome,
                            CleanupOutcome::Returned(CleanupObservation::Schedule {
                                enabled: false,
                                next_due_at: None,
                                active_run_id: None,
                                waiting_run_id: None,
                                ..
                            })
                        ),
                        "real schedule was not locally disabled",
                    )?;
                }
            }
            CleanupAction::Unsubscribe {
                actor,
                root_message_id,
            } => {
                *unsubscribe_count += 1;
                ensure(
                    expected.pairs.iter().any(|pair| {
                        pair.actor == *actor && pair.root_message_id == *root_message_id
                    }),
                    "unsubscribe targeted an unowned actor/root",
                )?;
                if !unavailable {
                    ensure(
                        matches!(
                            &attempt.outcome,
                            CleanupOutcome::Returned(CleanupObservation::Subscription {
                                state: ThreadSubscriptionState::Ended,
                                end_reason: Some(EndReason::Cancelled),
                                pending_count: 0,
                            })
                        ),
                        "explicit unsubscribe did not report Ended/Cancelled",
                    )?;
                }
            }
            CleanupAction::Unwatch {
                actor,
                root_message_id,
            } => {
                *unwatch_count += 1;
                ensure(
                    expected.pairs.iter().any(|pair| {
                        pair.actor == *actor && pair.root_message_id == *root_message_id
                    }),
                    "unwatch targeted an unowned actor/root",
                )?;
                if !unavailable {
                    ensure(
                        matches!(
                            &attempt.outcome,
                            CleanupOutcome::Returned(CleanupObservation::Watch { watching: false })
                        ),
                        "explicit unwatch did not report inactive Watch",
                    )?;
                }
            }
        }
        if unavailable {
            ensure(
                matches!(
                    &attempt.outcome,
                    CleanupOutcome::Unknown(_) | CleanupOutcome::Rejected(_)
                ),
                "unavailable Runtime cleanup fabricated a Returned/retired observation",
            )?;
        }
    }
    ensure(
        counts == expected_counts,
        "literal one-attempt cleanup counts changed",
    )?;
    ensure(
        report.attempts.len() == expected_counts.into_iter().sum::<usize>(),
        "cleanup attempt record was lost",
    )?;
    for operation_id in &cleanup_operations {
        ensure(
            cleanup_operations
                .iter()
                .filter(|actual| *actual == operation_id)
                .count()
                == 1,
            "cleanup reused an operation identity",
        )?;
    }
    for pair in &expected.pairs {
        let unsubscribes = report.attempts.iter().filter(|attempt| matches!(&attempt.action,
            CleanupAction::Unsubscribe { actor, root_message_id } if actor == &pair.actor && root_message_id == &pair.root_message_id)).count();
        let unwatches = report.attempts.iter().filter(|attempt| matches!(&attempt.action,
            CleanupAction::Unwatch { actor, root_message_id } if actor == &pair.actor && root_message_id == &pair.root_message_id)).count();
        ensure(
            unsubscribes == 1 && unwatches == 1,
            "actor/root action was replayed or omitted",
        )?;
    }
    Ok(())
}

async fn check_public_state(
    client: &CollaborationClient,
    expected: &ExpectedResources,
) -> ProofResult<()> {
    if let Some((_, wakeup_id)) = &expected.wake {
        let wake = client
            .read_wakeup(WakeShowRequest {
                wakeup_id: wakeup_id.clone(),
            })
            .await?;
        ensure(
            wake.definition.wakeup_id == *wakeup_id
                && wake.state == WakeState::Cancelled
                && wake.next_due_at.is_none(),
            "public wake read did not confirm exact Cancelled state",
        )?;
    }
    if let Some((_, schedule_id)) = &expected.schedule {
        let schedule = client
            .read_schedule(ScheduleShowRequest {
                schedule_id: schedule_id.clone(),
            })
            .await?;
        ensure(
            schedule.schedule_id == *schedule_id
                && !schedule.definition.enabled
                && schedule.next_due_at.is_none()
                && schedule.active_run_id.is_none()
                && schedule.waiting_run_id.is_none(),
            "public schedule read was not disabled",
        )?;
    }
    let mut checked_actors = Vec::new();
    for pair in &expected.pairs {
        if !checked_actors.contains(&&pair.actor) {
            let subscriptions = client
                .board_thread_subscriptions(ThreadSubscriptionsRequest {
                    actor: pair.actor.clone(),
                })
                .await?;
            ensure(
                subscriptions.subscriptions.is_empty(),
                "public active/draining subscription list was not empty",
            )?;
            checked_actors.push(&pair.actor);
        }
    }
    if let Some(pair) = expected.pairs.first() {
        let thread = client
            .board_thread_show(ThreadShowRequest {
                root_message_id: pair.root_message_id.clone(),
                reader: Some(pair.actor.clone()),
            })
            .await?;
        ensure(
            thread.thread.root_message_id == pair.root_message_id
                && thread.watch_status.is_some_and(|watch| !watch.watching),
            "public exact-reader Watch was not inactive",
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum StoreExpectation {
    Retired,
    Unchanged(RuntimePrefix),
}

async fn check_fixture_stores(
    fixture: &mut RuntimeCleanupFixture,
    expectation: StoreExpectation,
) -> ProofResult<()> {
    let board = fixture
        .board
        .as_mut()
        .ok_or("fixture-owned BoardStore was not opened")?;
    let automation = fixture
        .automation
        .as_mut()
        .ok_or("fixture-owned AutomationStore was not opened")?;
    check_store_state(board, automation, &fixture.expected, expectation).await
}
async fn check_store_state(
    board: &mut BoardStore,
    automation: &mut AutomationStore,
    expected: &ExpectedResources,
    expectation: StoreExpectation,
) -> ProofResult<()> {
    let retired = matches!(expectation, StoreExpectation::Retired);
    for pair in &expected.pairs {
        let record = board
            .get_thread_subscription_record(
                &pair.actor,
                &SubscriptionScope::thread(pair.root_message_id.clone()),
            )
            .await?
            .ok_or("exact actor/root subscription history disappeared")?;
        let state_matches = matches!(
            (retired, record.state()),
            (
                true,
                SubscriptionState::Ended {
                    reason: EndReason::Cancelled
                }
            ) | (false, SubscriptionState::Active)
        );
        ensure(
            state_matches,
            "exact SQLite subscription was not the literal expected local state",
        )?;
        let thread = board
            .show_thread(ThreadShowRequest {
                root_message_id: pair.root_message_id.clone(),
                reader: Some(pair.actor.clone()),
            })
            .await?;
        ensure(
            thread
                .watch_status
                .is_some_and(|watch| watch.watching != retired),
            "exact SQLite Watch state changed unexpectedly",
        )?;
    }
    if let Some((_, wakeup_id)) = &expected.wake {
        let wake = automation.read_wakeup::<SavedMessage>(wakeup_id).await?;
        let state = if retired {
            agent_automation::WakeState::Cancelled
        } else {
            agent_automation::WakeState::Active
        };
        ensure(
            wake.definition.wakeup_id == *wakeup_id
                && wake.state == state
                && wake.pending_delivery_id.is_none()
                && wake.next_due_at_ms.is_none() == retired,
            "SQLite wake did not preserve exact local state",
        )?;
    }
    if let Some((_, schedule_id)) = &expected.schedule {
        let schedule = automation
            .inspect_schedule::<SessionRef, EndpointRef>(schedule_id)
            .await?;
        let enabled = matches!(
            expectation,
            StoreExpectation::Unchanged(RuntimePrefix::EnabledSchedule)
        );
        ensure(
            schedule.record.schedule_id == *schedule_id
                && schedule.record.definition.enabled == enabled
                && schedule.record.next_due_at_ms.is_some() == enabled
                && schedule.active_run_id.is_none()
                && schedule.waiting_run_id.is_none(),
            "SQLite schedule changed or invented run cancellation",
        )?;
    }
    Ok(())
}

async fn check_reopened_stores(
    directory: &Path,
    expected: &ExpectedResources,
    expectation: StoreExpectation,
) -> ProofResult<()> {
    let mut board = None;
    let mut automation = None;
    let observed: ProofResult<()> = async {
        board = Some(BoardStore::open(&directory.join("project-board.sqlite")).await?);
        automation = Some(AutomationStore::open(&directory.join("automation.sqlite")).await?);
        check_store_state(
            board.as_mut().ok_or("reopened BoardStore missing")?,
            automation
                .as_mut()
                .ok_or("reopened AutomationStore missing")?,
            expected,
            expectation,
        )
        .await
    }
    .await;
    let board_close: ProofResult<()> = match board {
        Some(board) => board.close().await.map_err(Into::into),
        None => Ok(()),
    };
    let automation_close: ProofResult<()> = match automation {
        Some(automation) => automation.close().await.map_err(Into::into),
        None => Ok(()),
    };
    match (observed, board_close, automation_close) {
        (Ok(()), Ok(()), Ok(())) => Ok(()),
        (observed, board_close, automation_close) => Err(Box::new(ReopenFailure {
            observed: observed.err(),
            board_close: board_close.err(),
            automation_close: automation_close.err(),
        })),
    }
}

#[derive(Debug)]
struct InjectedPrefixFailure(RuntimePrefix);
impl fmt::Display for InjectedPrefixFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "injected L3 prefix failure: {:?}", self.0)
    }
}
impl std::error::Error for InjectedPrefixFailure {}
fn assess_scenario(
    prefix: RuntimePrefix,
    result: ProofResult<()>,
    cleanup_succeeded: bool,
) -> ProofResult<()> {
    let expected_error = matches!(
        prefix,
        RuntimePrefix::FirstReader | RuntimePrefix::WakeReceipt | RuntimePrefix::BeforePrepare
    );
    match result {
        Err(error)
            if cleanup_succeeded
                && expected_error
                && error
                    .downcast_ref::<InjectedPrefixFailure>()
                    .is_some_and(|error| error.0 == prefix) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
        Ok(()) if expected_error => Err("expected typed prefix failure was erased".into()),
        Ok(()) => Ok(()),
    }
}
type CaseError = Box<dyn std::error::Error + Send + Sync>;
#[derive(Debug)]
struct RuntimeCaseFailure {
    scenario: Option<CaseError>,
    stopped_api: Option<CaseError>,
    cleanup_observation: Option<CaseError>,
    teardown: Vec<FixtureCleanupIssue>,
    reopen: Option<CaseError>,
}
impl fmt::Display for RuntimeCaseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Runtime cleanup case failed (scenario={}, API stop={}, observation={}, teardown={}, reopen={})",
            self.scenario.is_some(),
            self.stopped_api.is_some(),
            self.cleanup_observation.is_some(),
            self.teardown.len(),
            self.reopen.is_some()
        )?;
        for issue in &self.teardown {
            write!(formatter, "; {:?} failed", issue.stage)?;
        }
        Ok(())
    }
}
impl std::error::Error for RuntimeCaseFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Some(error) = &self.scenario {
            return Some(error.as_ref());
        }
        if let Some(error) = &self.cleanup_observation {
            return Some(error.as_ref());
        }
        self.teardown
            .first()
            .map(|issue| issue.error.as_ref() as &(dyn std::error::Error + 'static))
    }
}
#[derive(Debug)]
struct ReopenFailure {
    observed: Option<CaseError>,
    board_close: Option<CaseError>,
    automation_close: Option<CaseError>,
}
impl fmt::Display for ReopenFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "fixture SQLite reopen failed (read={}, board close={}, automation close={})",
            self.observed.is_some(),
            self.board_close.is_some(),
            self.automation_close.is_some()
        )
    }
}
impl std::error::Error for ReopenFailure {}

#[tokio::test]
async fn actual_two_root_producer_uses_distinct_boards_and_preserves_participant_acl()
-> ProofResult<()> {
    use collaboration_client::{
        BoardClientError,
        board::{
            BoardFailureKind, Identity, MessageId, MessagePostRequest, MessageReferences,
            MessageShowRequest, Placement,
        },
        protocol::{EndpointId, SessionId},
    };
    let directory = tempfile::Builder::new()
        .prefix("u6-l3-producer-")
        .tempdir_in("/tmp")?;
    let mut fixture = RuntimeCleanupFixture::allocate(directory.path())?;
    let mut actors = None;
    let mut produced = None;
    let startup: ProofResult<()> = async {
        fixture.start().await?;
        let reader = fixture.peer_target()?;
        // Fixture identity only; this does not allocate or load a native session.
        let mut sender = reader.clone();
        sender.endpoint.endpoint_id = EndpointId::try_from("codex-local".to_owned())?;
        sender.session_id = SessionId::try_from("fixture-cleanup-sender".to_owned())?;
        actors = Some([
            serde_json::from_value::<Identity>(
                serde_json::json!({"kind":"session","session":sender}),
            )?,
            serde_json::from_value::<Identity>(
                serde_json::json!({"kind":"session","session":reader}),
            )?,
        ]);
        let owned = fixture
            .owned
            .as_mut()
            .ok_or("actual producer client missing")?;
        // Same actual client and authoritative pre-dispatch owner; no second accumulator.
        produced = Some(
            crate::held_subscription_producer::board_two_thread_subscription_push(
                &owned.client,
                &sender,
                &reader,
                ["L3-FIRST-ROOT-MARKER", "L3-SECOND-ROOT-MARKER"],
                &mut owned.resources,
            )
            .await,
        );
        Ok(())
    }
    .await;
    let prefix = match produced.as_ref() {
        Some(Ok(_)) => ActualHelperPrefix::Completed,
        Some(Err(error))
            if matches!(error.downcast_ref::<BoardClientError>(),
            Some(BoardClientError::Rejected(failure)) if failure.kind == BoardFailureKind::TopLevelMessageCooldown) =>
        {
            ActualHelperPrefix::Cooldown
        }
        _ => ActualHelperPrefix::Unqualified,
    };
    // Independent installed-row observations do not adopt, replace or trim cleanup ownership.
    let ownership_inspection = match &actors {
        Some(actors) => fixture.observe_actual_helper_pairs(actors).await,
        None => Err("actual helper actors were not established".into()),
    };
    let producer: ProofResult<()> = async {
        startup?;
        let roots = match produced.take().ok_or("actual helper was not invoked")? {
            Ok(roots) => roots,
            Err(error) => {
                let typed_cooldown = matches!(error.downcast_ref::<BoardClientError>(),
                    Some(BoardClientError::Rejected(failure)) if failure.kind == BoardFailureKind::TopLevelMessageCooldown);
                return Err(Box::new(ActualProducerFailure { typed_cooldown, cause: error }) as CaseError);
            }
        };
        let [first_root, second_root] = roots.as_slice() else { return Err("actual helper did not return exactly two roots".into()); };
        ensure(first_root != second_root && fixture.expected.pairs.len() == 4,
            "actual helper did not establish four distinct sender/reader root pairs")?;
        let board = fixture.board.as_mut().ok_or("fixture-owned board missing")?;
        let first = board.show_message(MessageShowRequest { message_id: first_root.clone() }).await?;
        let second = board.show_message(MessageShowRequest { message_id: second_root.clone() }).await?;
        ensure(first.0.message_id == *first_root && second.0.message_id == *second_root
            && first.0.board_id != second.0.board_id, "actual helper did not create roots on distinct boards")?;
        for actor in actors.as_ref().ok_or("actual helper actors missing")? {
            for root in roots.iter() {
                ensure(fixture.expected.pairs.iter().any(|pair| pair.actor == *actor && pair.root_message_id == *root),
                    "actual helper lost a sender/reader root subscription")?;
            }
        }
        let mut outsider = fixture.peer_target()?;
        outsider.session_id = SessionId::try_from("fixture-cleanup-outsider".to_owned())?;
        let outsider_actor = serde_json::from_value(serde_json::json!({"kind":"session","session":outsider}))?;
        let denied = fixture.owned.as_ref().ok_or("actual producer client missing")?.client.board_message_post(MessagePostRequest {
            message_id: MessageId::generate(), placement: Placement::Thread { root_message_id: first_root.clone() },
            actor: outsider_actor, acting_for: None,
            text: collaboration_client::board::MessageText::try_from("Rejected unjoined fixture post".to_owned())?,
            references: MessageReferences::try_from(Vec::new())?,
        }).await;
        ensure(matches!(denied, Err(BoardClientError::Rejected(failure)) if failure.kind == BoardFailureKind::ParticipantRequired),
            "unjoined fixture session bypassed thread participant ACL")
    }.await;
    let expected = fixture.expected.clone();
    let mut missing_pairs = Vec::new();
    let cleanup: ProofResult<()> = async {
        let owned = fixture
            .owned
            .take()
            .ok_or("actual producer cleanup owner missing")?;
        let plan = owned.resources.into_cleanup_plan();
        missing_pairs = missing_installed_pairs(&plan.history, &expected);
        let before_absence = check_fixture_absent_roots(&mut fixture, &missing_pairs).await;
        // Even failed absence/readback assertions never prevent any owned cleanup attempt.
        let report = cleanup_owned_resources(&owned.client, plan).await;
        let report_validation =
            check_actual_helper_report(&report, &expected, actors.as_ref(), prefix);
        let public_state = check_public_state(&owned.client, &expected).await;
        let store_state = check_fixture_stores(&mut fixture, StoreExpectation::Retired).await;
        let after_absence = check_fixture_absent_roots(&mut fixture, &missing_pairs).await;
        let not_returned = report
            .attempts
            .iter()
            .any(|attempt| !matches!(&attempt.outcome, CleanupOutcome::Returned(_)));
        if before_absence.is_ok()
            && report_validation.is_ok()
            && public_state.is_ok()
            && store_state.is_ok()
            && after_absence.is_ok()
            && !not_returned
            && report.unresolved.is_empty()
        {
            Ok(())
        } else {
            Err(Box::new(ActualProducerCleanupFailure {
                report,
                before_absence: before_absence.err(),
                report_validation: report_validation.err(),
                public_state: public_state.err(),
                store_state: store_state.err(),
                after_absence: after_absence.err(),
            }) as CaseError)
        }
    }
    .await;
    let teardown = fixture.finish().await;
    let reopen =
        check_reopened_stores(directory.path(), &expected, StoreExpectation::Retired).await;
    let absent_reopen = check_reopened_absent_roots(directory.path(), &missing_pairs).await;
    match (
        producer,
        ownership_inspection,
        cleanup,
        teardown,
        reopen,
        absent_reopen,
    ) {
        (Ok(()), Ok(()), Ok(()), teardown, Ok(()), Ok(())) if teardown.is_empty() => Ok(()),
        (producer, ownership_inspection, cleanup, teardown, reopen, absent_reopen) => {
            Err(Box::new(ActualProducerCaseFailure {
                producer: producer.err(),
                ownership_inspection: ownership_inspection.err(),
                cleanup: cleanup.err(),
                teardown,
                reopen: reopen.err(),
                absent_reopen: absent_reopen.err(),
            }) as CaseError)
        }
    }
}

#[derive(Debug)]
struct ActualProducerFailure {
    typed_cooldown: bool,
    cause: CaseError,
}
impl fmt::Display for ActualProducerFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.typed_cooldown {
            formatter.write_str("actual two-root helper rejected TopLevelMessageCooldown")
        } else {
            formatter.write_str("actual two-root helper failed before returning both roots")
        }
    }
}
impl std::error::Error for ActualProducerFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
#[derive(Debug)]
struct ActualProducerCaseFailure {
    producer: Option<CaseError>,
    ownership_inspection: Option<CaseError>,
    cleanup: Option<CaseError>,
    teardown: Vec<FixtureCleanupIssue>,
    reopen: Option<CaseError>,
    absent_reopen: Option<CaseError>,
}
impl fmt::Display for ActualProducerCaseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "actual producer case failed (producer={}, ownership={}, cleanup={}, teardown={}, reopen={}, absent reopen={})",
            self.producer.is_some(),
            self.ownership_inspection.is_some(),
            self.cleanup.is_some(),
            self.teardown.len(),
            self.reopen.is_some(),
            self.absent_reopen.is_some()
        )
    }
}
impl std::error::Error for ActualProducerCaseFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.producer
            .as_ref()
            .map(|error| error.as_ref() as &(dyn std::error::Error + 'static))
    }
}

async fn check_fixture_absent_roots(
    fixture: &mut RuntimeCleanupFixture,
    pairs: &[ActorRootExpectation],
) -> ProofResult<()> {
    check_absent_roots(
        fixture
            .board
            .as_mut()
            .ok_or("fixture-owned BoardStore missing")?,
        pairs,
    )
    .await
}
async fn check_absent_roots(
    board: &mut BoardStore,
    pairs: &[ActorRootExpectation],
) -> ProofResult<()> {
    use collaboration_client::board::{BoardFailureKind, BoardFailureStage, MessageShowRequest};
    for pair in pairs {
        ensure(
            board
                .get_thread_subscription_record(
                    &pair.actor,
                    &SubscriptionScope::thread(pair.root_message_id.clone()),
                )
                .await?
                .is_none(),
            "never-created helper subscription appeared",
        )?;
        let message = board
            .show_message(MessageShowRequest {
                message_id: pair.root_message_id.clone(),
            })
            .await;
        ensure(
            matches!(message, Err(error) if error.kind == BoardFailureKind::ResourceNotFound
            && error.stage == BoardFailureStage::Inspection),
            "never-created helper root appeared or absence read was inconclusive",
        )?;
    }
    Ok(())
}
async fn check_reopened_absent_roots(
    directory: &Path,
    pairs: &[ActorRootExpectation],
) -> ProofResult<()> {
    let mut board = BoardStore::open(&directory.join("project-board.sqlite")).await?;
    let observed = check_absent_roots(&mut board, pairs).await;
    let board_close: ProofResult<()> = board.close().await.map_err(Into::into);
    match (observed, board_close) {
        (Ok(()), Ok(())) => Ok(()),
        (observed, board_close) => Err(Box::new(ReopenFailure {
            observed: observed.err(),
            board_close: board_close.err(),
            automation_close: None,
        })),
    }
}

#[derive(Debug)]
struct ActualProducerCleanupFailure {
    report: MatrixCleanupReport,
    before_absence: Option<CaseError>,
    report_validation: Option<CaseError>,
    public_state: Option<CaseError>,
    store_state: Option<CaseError>,
    after_absence: Option<CaseError>,
}
impl fmt::Display for ActualProducerCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "actual helper cleanup report: {:?}; evidence failures (before absence={}, report={}, public={}, store={}, after absence={})",
            self.report,
            self.before_absence.is_some(),
            self.report_validation.is_some(),
            self.public_state.is_some(),
            self.store_state.is_some(),
            self.after_absence.is_some()
        )
    }
}
impl std::error::Error for ActualProducerCleanupFailure {}
