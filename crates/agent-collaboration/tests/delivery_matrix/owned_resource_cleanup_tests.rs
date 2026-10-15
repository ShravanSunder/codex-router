//! Real API/service/SQLite case; no native endpoint, HOME override or mocked cleanup call.
use super::super::{owned_resources::OwnedMatrixResources, proof_context::ProofResult};
use super::{CleanupAction, CleanupObservation, CleanupOutcome, cleanup_owned_resources};
use automation_storage::AutomationStore;
use collaboration_client::protocol::{
    EndpointRef, InstructionCreateParams, OperationId, SavedMessage, ScheduleCreateRequest,
    ScheduleId, ScheduleShowRequest, SessionRef, WakeSendRequest, WakeShowRequest, WakeState,
    WakeupId,
};
use collaboration_mcp::test_support::ServedCollaborationApi;
use collaboration_service::{CollaborationApplication, ServiceIdentity};
use serde_json::json;
use std::{fmt, os::unix::fs::PermissionsExt as _, sync::Arc};
use tokio::sync::Mutex;

#[tokio::test]
async fn known_timer_cleanup_reaches_real_service_and_sqlite() -> ProofResult<()> {
    let directory = tempfile::Builder::new()
        .prefix("cleanup-")
        .tempdir_in("/tmp")?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let database_path = directory.path().join("automation.sqlite");
    let store = Arc::new(Mutex::new(AutomationStore::open(&database_path).await?));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000401",
        "00000000-0000-4000-8000-000000000402",
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(Arc::clone(&store));
    let served =
        ServedCollaborationApi::start(directory.path(), CollaborationApplication::new(identity))
            .await?;

    let scenario = exercise_real_timer_cleanup(&served, &store).await;
    // These are awaited even when any scenario oracle returned an error.
    let api_stop = served.stop().await;
    let storage_close = match Arc::try_unwrap(store) {
        Ok(store) => store.into_inner().close().await.map_err(Into::into),
        Err(_) => Err("real API retained automation SQLite after awaited stop".into()),
    };
    let (wakeup_id, schedule_id) = finish_fixture(scenario, api_stop, storage_close)?;

    let mut reopened = AutomationStore::open(&database_path).await?;
    let observed = check_stored_timer_retirement(&mut reopened, &wakeup_id, &schedule_id).await;
    let storage_close = reopened.close().await.map_err(Into::into);
    finish_fixture(observed, Ok(()), storage_close)
}

async fn exercise_real_timer_cleanup(
    served: &ServedCollaborationApi,
    store: &Arc<Mutex<AutomationStore>>,
) -> ProofResult<(WakeupId, ScheduleId)> {
    let client = served.client("owned-cleanup-test").await?;
    let mut owner = OwnedMatrixResources::default();
    let wake_operation = OperationId::generate();
    owner.record_wake_submission(wake_operation.clone())?;
    let wake_request: WakeSendRequest = serde_json::from_value(json!({
        "operationId": wake_operation,
        "message": {
            "target": {
                "endpoint": {
                    "serviceId": "00000000-0000-4000-8000-000000000401",
                    "endpointId": "codex-local"
                },
                "sessionId": "cleanup-fixture-only-target"
            },
            "content": {"kind": "humanUser", "text": "Synthetic cleanup fixture"},
            "delivery": "queue", "generationGuard": null
        },
        "timing": {"kind": "after", "seconds": 600},
        "expiry": {"kind": "none"}
    }))?;
    let wake = client.send_wakeup(wake_request).await?;
    owner.record_wake_receipt(&wake_operation, wake.definition.wakeup_id.clone())?;

    let instruction_operation = OperationId::generate();
    owner.record_instruction_submission(instruction_operation.clone())?;
    let instruction = client
        .create_instruction(InstructionCreateParams {
            operation_id: instruction_operation.clone(),
            text: "Synthetic schedule instruction".to_owned().try_into()?,
        })
        .await?;
    owner.record_instruction_receipt(&instruction_operation, instruction.instruction_id.clone())?;
    let schedule_operation = OperationId::generate();
    owner.record_schedule_submission(schedule_operation.clone())?;
    let schedule_request: ScheduleCreateRequest = serde_json::from_value(json!({
        "operationId": schedule_operation,
        "definition": {
            "instructionId": instruction.instruction_id,
            "timing": {"kind": "interval", "seconds": 600}, "enabled": false,
            "destination": {"kind": "unprepared"}, "executionTimeoutSeconds": null,
            "model": "fixture-model", "effort": "medium"
        }
    }))?;
    let schedule = client.create_schedule(schedule_request).await?;
    owner.record_schedule_receipt(&schedule_operation, schedule.schedule_id.clone())?;

    let report = cleanup_owned_resources(&client, owner.into_cleanup_plan()).await;
    let [wake_attempt, schedule_attempt] = report.attempts.as_slice() else {
        return Err("expected exactly two owned timer cleanup attempts".into());
    };
    if !report.unresolved.is_empty() {
        return Err("known real timer receipts became unknown creations".into());
    }
    match (&wake_attempt.action, &wake_attempt.outcome) {
        (
            CleanupAction::CancelWake {
                operation_id,
                wakeup_id,
            },
            CleanupOutcome::Returned(CleanupObservation::Wake {
                state: WakeState::Cancelled,
                next_due_at: None,
                pending_delivery_id: None,
                discarded_delivery_ids,
                retained_deliveries,
            }),
        ) if operation_id != &wake_operation
            && wakeup_id == &wake.definition.wakeup_id
            && discarded_delivery_ids.is_empty()
            && retained_deliveries.is_empty() => {}
        _ => {
            return Err(
                "real wake cleanup receipt did not report exact Cancelled ownership".into(),
            );
        }
    }
    let cleanup_change_id = match (&schedule_attempt.action, &schedule_attempt.outcome) {
        (
            CleanupAction::DisableSchedule {
                operation_id,
                schedule_id,
            },
            CleanupOutcome::Returned(CleanupObservation::Schedule {
                enabled: false,
                next_due_at: None,
                active_run_id: None,
                waiting_run_id: None,
                change_id,
            }),
        ) if operation_id != &schedule_operation && schedule_id == &schedule.schedule_id => {
            change_id.clone()
        }
        _ => {
            return Err(
                "real schedule cleanup receipt did not report exact disabled ownership".into(),
            );
        }
    };
    let wake_read = client
        .read_wakeup(WakeShowRequest {
            wakeup_id: wake.definition.wakeup_id.clone(),
        })
        .await?;
    let schedule_read = client
        .read_schedule(ScheduleShowRequest {
            schedule_id: schedule.schedule_id.clone(),
        })
        .await?;
    if wake_read.definition.wakeup_id != wake.definition.wakeup_id
        || wake_read.state != WakeState::Cancelled
        || wake_read.next_due_at.is_some()
        || schedule_read.schedule_id != schedule.schedule_id
        || schedule_read.change_id != cleanup_change_id
        || schedule_read.definition.enabled
        || schedule_read.next_due_at.is_some()
    {
        return Err("public reads did not confirm exact timer retirement".into());
    }
    check_stored_timer_retirement(
        &mut *store.lock().await,
        &wake.definition.wakeup_id,
        &schedule.schedule_id,
    )
    .await?;
    Ok((wake.definition.wakeup_id, schedule.schedule_id))
}

async fn check_stored_timer_retirement(
    store: &mut AutomationStore,
    wakeup_id: &WakeupId,
    schedule_id: &ScheduleId,
) -> ProofResult<()> {
    let wake = store.read_wakeup::<SavedMessage>(wakeup_id).await?;
    let schedule = store
        .inspect_schedule::<SessionRef, EndpointRef>(schedule_id)
        .await?;
    if wake.definition.wakeup_id != *wakeup_id
        || wake.state != agent_automation::WakeState::Cancelled
        || wake.next_due_at_ms.is_some()
        || schedule.record.schedule_id != *schedule_id
        || schedule.record.definition.enabled
        || schedule.record.next_due_at_ms.is_some()
        || schedule.active_run_id.is_some()
        || schedule.waiting_run_id.is_some()
    {
        return Err("SQLite did not retain exact Cancelled/disabled timer state".into());
    }
    Ok(())
}

#[derive(Debug)]
struct FixtureCleanupFailure {
    scenario: Option<Box<dyn std::error::Error + Send + Sync>>,
    api_stop: Option<std::io::Error>,
    storage_close: Option<Box<dyn std::error::Error + Send + Sync>>,
}
impl fmt::Display for FixtureCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "real cleanup fixture failed (scenario={}, API stop={}, storage close={})",
            self.scenario.is_some(),
            self.api_stop.is_some(),
            self.storage_close.is_some()
        )
    }
}
impl std::error::Error for FixtureCleanupFailure {}

fn finish_fixture<TValue>(
    scenario: ProofResult<TValue>,
    api_stop: std::io::Result<()>,
    storage_close: ProofResult<()>,
) -> ProofResult<TValue> {
    match (scenario, api_stop, storage_close) {
        (Ok(value), Ok(()), Ok(())) => Ok(value),
        (scenario, api_stop, storage_close) => Err(Box::new(FixtureCleanupFailure {
            scenario: scenario.err(),
            api_stop: api_stop.err(),
            storage_close: storage_close.err(),
        })),
    }
}

#[test]
fn wake_failure_projection_preserves_kind_stage_known_ids_and_commit_state() -> ProofResult<()> {
    use super::{ReportedCleanupFailure, WakeCleanupFailureKind};
    use collaboration_client::{
        WakeClientError,
        protocol::{
            LocalMutationEvidence, LocalMutationState, WakeFailure, WakeFailureReason,
            WakeFailureStage, WakeNextAction,
        },
    };
    let operation_id = OperationId::try_from("019f0000-0000-7000-8000-000000009001".to_owned())?;
    let wakeup_id = WakeupId::try_from("019f0000-0000-7000-8000-000000009011".to_owned())?;
    let failure = WakeFailure {
        reason: WakeFailureReason::InvalidRecord,
        stage: WakeFailureStage::Storage,
        message: "UNRETAINED-SYNTHETIC-WAKE-TEXT".to_owned(),
        operation_id: Some(operation_id.clone()),
        wakeup_id: Some(wakeup_id.clone()),
        effects: LocalMutationEvidence::Local {
            mutation: LocalMutationState::Committed,
        },
        next_action: WakeNextAction::InspectOperation,
    };
    let outcome = super::project_wake_failure(WakeClientError::Rejected(Box::new(failure)));
    if !matches!(&outcome, CleanupOutcome::Rejected(ReportedCleanupFailure::Wake {
        kind: WakeCleanupFailureKind::InvalidRecord, stage: WakeFailureStage::Storage,
        operation_id: Some(actual_operation), wakeup_id: Some(actual_wakeup), mutation: LocalMutationState::Committed,
    }) if actual_operation == &operation_id && actual_wakeup == &wakeup_id)
    {
        return Err(
            "wake rejection lost or rebound reported kind/stage/known IDs/committed effect".into(),
        );
    }
    let absent = WakeFailure {
        reason: WakeFailureReason::InvalidField {
            field: "UNRETAINED-FIELD".into(),
            constraint: "UNRETAINED-CONSTRAINT".into(),
        },
        stage: WakeFailureStage::Validation,
        message: "UNRETAINED-SYNTHETIC-WAKE-TEXT".into(),
        operation_id: None,
        wakeup_id: None,
        effects: LocalMutationEvidence::Local {
            mutation: LocalMutationState::None,
        },
        next_action: WakeNextAction::CorrectRequest,
    };
    let absent = super::project_wake_failure(WakeClientError::Rejected(Box::new(absent)));
    if !matches!(
        absent,
        CleanupOutcome::Rejected(ReportedCleanupFailure::Wake {
            kind: WakeCleanupFailureKind::InvalidField,
            stage: WakeFailureStage::Validation,
            operation_id: None,
            wakeup_id: None,
            mutation: LocalMutationState::None,
        })
    ) {
        return Err("wake validation failure invented identities or a committed effect".into());
    }
    if format!("{outcome:?}").contains("UNRETAINED") {
        return Err("wake projection retained arbitrary failure text".into());
    }
    Ok(())
}

#[test]
fn schedule_failure_projection_preserves_unknown_local_effect_and_route_kind() -> ProofResult<()> {
    use super::{ReportedCleanupEffect, ReportedCleanupFailure};
    use collaboration_client::{
        ScheduleClientError,
        protocol::{
            CessationEvidence, ChangeId, DeliveryRouteEvidence, LocalMutationState,
            NativeEffectEvidence, PreparationEffect, ScheduleEffects, ScheduleFailure,
            ScheduleFailureDetails, ScheduleFailureKind, ScheduleFailureStage, ScheduleNextAction,
            SubmissionEffect,
        },
    };
    let operation_id = OperationId::try_from("019f0000-0000-7000-8000-000000009002".to_owned())?;
    let schedule_id = ScheduleId::try_from("019f0000-0000-7000-8000-000000009012".to_owned())?;
    let change_id = ChangeId::try_from("019f0000-0000-7000-8000-000000009013".to_owned())?;
    let failure = ScheduleFailure {
        kind: ScheduleFailureKind::OutcomeUnknown,
        stage: ScheduleFailureStage::Storage,
        message: "UNRETAINED-SYNTHETIC-SCHEDULE-TEXT".into(),
        operation_id: Some(operation_id.clone()),
        schedule_id: Some(schedule_id.clone()),
        current_change_id: Some(change_id.clone()),
        field: None,
        constraint: None,
        details: ScheduleFailureDetails::None,
        effects: ScheduleEffects::Local {
            mutation: LocalMutationState::Unknown,
        },
        next_action: ScheduleNextAction::InspectOperation,
    };
    let local =
        super::project_schedule_failure(ScheduleClientError::Rejected(Box::new(failure.clone())));
    if !matches!(&local, CleanupOutcome::Rejected(ReportedCleanupFailure::Schedule {
        kind: ScheduleFailureKind::OutcomeUnknown, stage: ScheduleFailureStage::Storage,
        operation_id: Some(actual_operation), schedule_id: Some(actual_schedule), current_change_id: Some(actual_change),
        effect: ReportedCleanupEffect::Local(LocalMutationState::Unknown),
    }) if actual_operation == &operation_id && actual_schedule == &schedule_id && actual_change == &change_id)
    {
        return Err(
            "schedule rejection lost known IDs or laundered reported unknown local mutation".into(),
        );
    }
    let route = ScheduleFailure {
        effects: ScheduleEffects::Route {
            evidence: DeliveryRouteEvidence::CodexAppServer(NativeEffectEvidence {
                target: None,
                generation: None,
                client_user_message_id: None,
                native_turn_id: None,
                native_submission_id: None,
                allocation: PreparationEffect::NotRequested,
                resume: PreparationEffect::Unknown,
                submission: SubmissionEffect::Unknown,
                cessation: CessationEvidence::Unconfirmed,
            }),
        },
        ..failure
    };
    let route = super::project_schedule_failure(ScheduleClientError::Rejected(Box::new(route)));
    if !matches!(&route, CleanupOutcome::Rejected(ReportedCleanupFailure::Schedule {
        kind: ScheduleFailureKind::OutcomeUnknown, stage: ScheduleFailureStage::Storage,
        operation_id: Some(actual_operation), schedule_id: Some(actual_schedule), current_change_id: Some(actual_change),
        effect: ReportedCleanupEffect::Route,
    }) if actual_operation == &operation_id && actual_schedule == &schedule_id && actual_change == &change_id)
    {
        return Err(
            "schedule route uncertainty became local/no-effect evidence or changed IDs".into(),
        );
    }
    if format!("{local:?}{route:?}").contains("UNRETAINED") {
        return Err("schedule projection retained arbitrary failure text".into());
    }
    Ok(())
}

#[test]
fn board_failure_projection_keeps_typed_rejection_separate_from_unknown_write() -> ProofResult<()> {
    use super::{CleanupUncertainty, ReportedCleanupFailure};
    use collaboration_client::{
        BoardClientError,
        board::{
            BoardError, BoardFailureKind, BoardFailureStage, BoardNextAction, MessageId,
            ResourceIdentity,
        },
    };
    let root_message_id = MessageId::try_from("019f0000-0000-7000-8000-000000009014".to_owned())?;
    let resource = ResourceIdentity::Thread { root_message_id };
    let rejected = super::project_board_failure(BoardClientError::Rejected(Box::new(
        BoardError::resource_not_found(resource.clone()),
    )));
    if !matches!(
        rejected,
        CleanupOutcome::Rejected(ReportedCleanupFailure::Board {
            kind: BoardFailureKind::ResourceNotFound,
            stage: BoardFailureStage::Inspection,
        })
    ) {
        return Err("board rejection lost actual kind/stage or became unknown/success".into());
    }
    let unknown = super::project_board_failure(BoardClientError::OutcomeUnknown {
        resource,
        message: "UNRETAINED-SYNTHETIC-BOARD-TEXT",
        next_action: BoardNextAction::InspectResource,
    });
    if !matches!(
        unknown,
        CleanupOutcome::Unknown(CleanupUncertainty::BoardOutcomeUnknown)
    ) {
        return Err(
            "uncertain board write was laundered into rejected/returned/no-effect evidence".into(),
        );
    }
    Ok(())
}

#[test]
fn client_failure_projection_preserves_transport_kind_and_exact_rpc_code() -> ProofResult<()> {
    use super::{CleanupUncertainty, ClientFailureKind};
    use collaboration_client::ClientError;
    let transport = super::project_client_failure(ClientError::Transport(
        std::io::ErrorKind::ConnectionReset.into(),
    ));
    if !matches!(
        transport,
        CleanupOutcome::Unknown(CleanupUncertainty::Client(ClientFailureKind::Transport(
            std::io::ErrorKind::ConnectionReset
        ),))
    ) {
        return Err(
            "transport uncertainty lost its closed I/O category or became a rejection".into(),
        );
    }
    let rpc = super::project_client_failure(ClientError::Rejected {
        code: -32603,
        data: Some(json!({"privateText":"UNRETAINED-SYNTHETIC-RPC-TEXT"})),
    });
    if !matches!(
        &rpc,
        CleanupOutcome::Unknown(CleanupUncertainty::Client(ClientFailureKind::RpcRejected {
            code: -32603
        }))
    ) {
        return Err(
            "RPC cleanup uncertainty lost its exact signed code or became a returned outcome"
                .into(),
        );
    }
    if format!("{rpc:?}").contains("UNRETAINED") {
        return Err("client projection retained arbitrary RPC data".into());
    }
    Ok(())
}

#[test]
fn wake_response_projection_keeps_retained_delivery_identity_and_disposition() -> ProofResult<()> {
    use super::CleanupUncertainty;
    use collaboration_client::protocol::{DeliveryDisposition, WakeMutationResult};
    let expected_wakeup = WakeupId::try_from("019f0000-0000-7000-8000-000000009011".to_owned())?;
    let result: WakeMutationResult = serde_json::from_value(json!({
        "wakeup":{"definition":{"wakeupId":"019f0000-0000-7000-8000-000000009011",
            "changeId":"019f0000-0000-7000-8000-000000009013",
            "message":{"target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000401","endpointId":"codex-local"},"sessionId":"cleanup-fixture-only-target"},
                "content":{"kind":"humanUser","text":"Synthetic retained-delivery boundary"},"delivery":"queue"},
            "timing":{"kind":"after","seconds":600},"anchorAt":"2026-10-15T00:00:00Z","expiresAt":null,"createdAt":"2026-10-15T00:00:00Z"},
            "state":"cancelled","nextDueAt":null,"firstFire":null,"pendingDeliveryId":null,"latestEventCursor":"synthetic:7"},
        "discardedDeliveryIds":["019f0000-0000-7000-8000-000000009023"],
        "dispatchedDeliveries":[
            {"deliveryId":"019f0000-0000-7000-8000-000000009021",
             "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000401","endpointId":"codex-local"},"sessionId":"cleanup-fixture-only-target"},
             "mode":"queue","source":{"kind":"wake","wakeupId":"019f0000-0000-7000-8000-000000009011","occurrenceId":"019f0000-0000-7000-8000-000000009020"},
             "eligibleAt":"2026-10-15T00:00:01Z","expiresAt":null,"disposition":"accepted",
             "evidence":{"kind":"accepted","attemptId":"019f0000-0000-7000-8000-000000009031",
                 "receipt":{"outcome":{"kind":"queued"},"reachability":"codexAppServer","client":null}}},
            {"deliveryId":"019f0000-0000-7000-8000-000000009022",
             "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000401","endpointId":"codex-local"},"sessionId":"cleanup-fixture-only-target"},
             "mode":"auto","source":{"kind":"wake","wakeupId":"019f0000-0000-7000-8000-000000009011","occurrenceId":"019f0000-0000-7000-8000-000000009020"},
             "eligibleAt":"2026-10-15T00:00:01Z","expiresAt":null,"disposition":"uncertain",
             "evidence":{"kind":"outcomeUnknown","attemptId":"019f0000-0000-7000-8000-000000009032",
                 "effects":{"kind":"codexAppServer","target":null,"generation":null,"clientUserMessageId":null,"nativeTurnId":null,"nativeSubmissionId":null,
                    "allocation":"notRequested","resume":"unknown","submission":"unknown","cessation":"unconfirmed"},"explanation":"Synthetic unknown dispatch","receipt":null}}
        ]
    }))?;
    let wrong_wakeup = WakeupId::try_from("019f0000-0000-7000-8000-000000009099".to_owned())?;
    if !matches!(
        super::project_wake_result(&wrong_wakeup, result.clone()),
        CleanupOutcome::Unknown(CleanupUncertainty::ResponseIdentityMismatch)
    ) {
        return Err("wake response was rebound to a different requested resource".into());
    }
    let outcome = super::project_wake_result(&expected_wakeup, result);
    let CleanupOutcome::Returned(CleanupObservation::Wake {
        state: WakeState::Cancelled,
        next_due_at: None,
        pending_delivery_id: None,
        discarded_delivery_ids,
        retained_deliveries,
    }) = outcome
    else {
        return Err("matching wake mutation lost its reported local state".into());
    };
    let expected_discarded = collaboration_client::protocol::DeliveryId::try_from(
        "019f0000-0000-7000-8000-000000009023".to_owned(),
    )?;
    if discarded_delivery_ids.as_slice() != [expected_discarded] {
        return Err("discarded delivery identity changed".into());
    }
    let [accepted, uncertain] = retained_deliveries.as_slice() else {
        return Err(
            "retained dispatched delivery evidence was dropped, duplicated or reordered".into(),
        );
    };
    let expected_accepted = collaboration_client::protocol::DeliveryId::try_from(
        "019f0000-0000-7000-8000-000000009021".to_owned(),
    )?;
    let expected_uncertain = collaboration_client::protocol::DeliveryId::try_from(
        "019f0000-0000-7000-8000-000000009022".to_owned(),
    )?;
    if accepted.delivery_id != expected_accepted
        || !matches!(accepted.disposition, DeliveryDisposition::Accepted)
        || uncertain.delivery_id != expected_uncertain
        || !matches!(uncertain.disposition, DeliveryDisposition::Uncertain)
    {
        return Err(
            "retained delivery IDs or accepted/uncertain dispositions were rebound or laundered"
                .into(),
        );
    }
    Ok(())
}

#[path = "owned_resource_cleanup_runtime_tests.rs"]
mod runtime_tests;
