//! Live scheduled exclusion, ordinary steering and unloaded-queue preconditions.
use super::proof_context::{ProofContext, ProofResult};
use codex_native_integration::NativeOperation;
use collaboration_client::protocol::{
    DeliveryClientReceipt, DeliveryReceipt, DeliveryRouteEvidence, DestinationPreparation,
    ExecutionDestination, InstructionCreateParams, MessageContent, MessageDelivery,
    NativeSendAcceptance, OperationId, RunExecution, RunListRequest, RunState,
    ScheduleCreateRequest, ScheduleDefinition, ScheduleEnableRequest, SchedulePrepareRequest,
    SessionMessageSendParams, SessionRef, TimingRequest,
};
use collaboration_client::{ClientError, ControlClient};
use serde_json::{Value, json};
use std::time::Duration;

pub async fn exercise() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let target = proof.start_thread("Luna busy recipient").await?;
    let idle_rejection = send_agent_probe(MessageProbeInput {
        proof: &proof,
        target: &target,
        delivery: MessageDelivery::Steer,
        text: "This idle steer must be rejected without starting work.",
    })
    .await;
    require_rejection(idle_rejection, "noActiveTurn")?;
    proof.record("idleSteerRejected", json!({"target":target}))?;
    let initial = send_agent_probe(MessageProbeInput {
        proof: &proof, target: &target, delivery: MessageDelivery::Auto,
        text: "Run exactly /bin/sleep 20 through the shell tool. Wait for that command to finish, then output exactly BUSY_COMPLETE. Do not start other work or spawn agents.",
    }).await?;
    let Some(DeliveryClientReceipt::CodexAppServer(native)) = &initial.client else {
        return Err("Auto on the fresh idle thread lacked native client evidence".into());
    };
    let NativeSendAcceptance::NativeInputAccepted { turn_id, .. } = &native.acceptance else {
        return Err("Auto on the fresh idle thread did not start a native turn".into());
    };
    let initial_turn_id = String::from(turn_id.clone());
    proof.record("idleAutoStarted", json!(initial))?;
    let instruction = proof
        .client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: "Output exactly SCHEDULED_AFTER_BUSY. Do not call tools or spawn agents."
                .to_owned()
                .try_into()?,
        })
        .await?;
    let schedule = proof
        .client
        .create_schedule(ScheduleCreateRequest {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRequest::After {
                    seconds: 5.try_into()?,
                },
                enabled: false,
                destination: ExecutionDestination::Unprepared,
                execution_timeout_seconds: Some(120.try_into()?),
                model: Some("gpt-5.6-sol".into()),
                effort: Some("medium".into()),
            },
        })
        .await?;
    proof
        .client
        .prepare_schedule(SchedulePrepareRequest {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id.clone(),
            destination: DestinationPreparation::Existing {
                target: target.clone(),
                cwd: proof.workspace.to_string_lossy().into_owned(),
            },
        })
        .await?;
    let schedule = proof
        .client
        .enable_schedule(ScheduleEnableRequest {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id,
        })
        .await?;
    if schedule.next_due_at.is_none() {
        return Err(
            "Busy proof missed its one-shot timing during setup; no scheduled input was submitted"
                .into(),
        );
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    let mut busy_observations = 0;
    let mut sent_note = false;
    let finished_run = loop {
        interval.tick().await;
        let page = proof
            .client
            .list_runs(RunListRequest {
                schedule_id: schedule.schedule_id.clone(),
                cursor: None,
                limit: 100.try_into()?,
            })
            .await?;
        if page.records.len() > 1 || page.next_cursor.is_some() {
            return Err("One-shot schedule created additional Runs".into());
        }
        if let Some(run) = page.records.first() {
            match &run.state {
                RunState::Preparing { .. }
                    if run
                        .execution_evidence
                        .route
                        .as_ref()
                        .and_then(|route| match route {
                            DeliveryRouteEvidence::CodexAppServer(native) => native.target.as_ref(),
                            _ => None,
                        })
                        == Some(&target) =>
                {
                    let turns = proof.turns(&target).await?;
                    if turns.iter().any(|turn| {
                        turn.get("id").and_then(Value::as_str) == Some(initial_turn_id.as_str())
                            && turn.get("status").and_then(Value::as_str) == Some("inProgress")
                    }) {
                        if run.execution_evidence.timing.is_some()
                            || run.execution_evidence.acceptance.is_some()
                        {
                            return Err("Known busy preparation consumed an execution budget or recorded a submission".into());
                        }
                        busy_observations += 1;
                        if busy_observations >= 2 && !sent_note {
                            proof.record("scheduledRunWaitsForBusyThread", json!(run))?;
                            let note = send_agent_probe(MessageProbeInput {
                                proof: &proof, target: &target, delivery: MessageDelivery::Auto,
                                text: "Informational agent input: this adds no new task. Continue the current task unless it is interrupted. Do not start any additional work.",
                            }).await?;
                            if !matches!(&note.client, Some(DeliveryClientReceipt::CodexAppServer(native))
                            if matches!(&native.acceptance, NativeSendAcceptance::SteerAccepted { turn_id, .. }
                            if String::from(turn_id.clone()) == initial_turn_id)
                            ) {
                                proof.record("busyNoteUnexpectedReceipt", json!(note))?;
                                return Err("Auto message did not steer the exact active turn while the schedule waited".into());
                            }
                            proof.record("ordinaryMessageSteeredBusyThread", json!(note))?;
                            let interrupted = proof
                                .client
                                .interrupt_turn(&target, &proof.generation, &initial_turn_id)
                                .await?;
                            proof
                                .record("externalTurnInterruptionRequested", json!(interrupted))?;
                            sent_note = true;
                        }
                    }
                }
                RunState::Finished {
                    execution, outcome, ..
                } => {
                    let RunExecution::CodexAppServer(execution) = execution else {
                        return Err("Busy-thread schedule did not use the native route".into());
                    };
                    if !sent_note
                        || busy_observations < 2
                        || execution.target != target
                        || execution.turn_id == initial_turn_id
                        || run.summary.is_some()
                        || !matches!(
                            outcome,
                            collaboration_client::protocol::WorkerOutcome::Completed { .. }
                        )
                    {
                        return Err("Scheduled continuation did not preserve busy waiting, ordinary messaging and a distinct worker turn".into());
                    }
                    let turns = proof.wait_for_text(&target, "SCHEDULED_AFTER_BUSY").await?;
                    if !turns.iter().any(|turn| {
                        turn.get("id").and_then(Value::as_str) == Some(initial_turn_id.as_str())
                            && matches!(
                                turn.get("status").and_then(Value::as_str),
                                Some("interrupted" | "completed" | "failed")
                            )
                    }) {
                        return Err(
                            "Original turn cessation was not confirmed by exact native history"
                                .into(),
                        );
                    }
                    if !turns.iter().any(|turn| {
                        turn.get("id").and_then(Value::as_str) == Some(execution.turn_id.as_str())
                            && super::proof_context::agent_text(turn)
                                .contains("SCHEDULED_AFTER_BUSY")
                    }) {
                        return Err(
                            "Scheduled output was not authored in the recorded worker turn".into(),
                        );
                    }
                    proof.record("scheduledBusyExclusionVerified", json!(run))?;
                    break run.clone();
                }
                RunState::PreparationFailed { .. }
                | RunState::SummaryBlocked { .. }
                | RunState::Uncertain { .. } => {
                    proof.record("busyScheduleFailed", json!(run))?;
                    return Err("Busy-thread schedule failed; inspect private Run evidence".into());
                }
                _ => {}
            }
        }
        if tokio::time::Instant::now() >= deadline {
            proof.record("busyScheduleWaitExpired", json!({"runs":page.records}))?;
            return Err("Scheduled continuation did not complete within the proof deadline".into());
        }
    };
    let thread_id = String::from(target.session_id.clone());
    // Codex auto-attaches initialized clients, including the Host observer, to
    // created threads. Replace only our owned backend to establish unloaded state.
    super::debug_backend_restart::restart(&mut proof).await?;
    let recovered = proof
        .client
        .read_run(collaboration_client::protocol::RunShowRequest {
            run_id: finished_run.run_id.clone(),
        })
        .await?;
    if serde_json::to_value(&recovered)? != serde_json::to_value(&finished_run)? {
        return Err("Backend replacement changed the completed Run's retained evidence".into());
    }
    if is_loaded(&mut proof, &thread_id).await? {
        return Err(
            "Fresh backend unexpectedly loaded the saved test thread; queue proof not attempted"
                .into(),
        );
    }
    proof.record("completedRunSurvivedBackendReplacement", json!(recovered))?;
    require_rejection(
        send_agent_probe(MessageProbeInput {
            proof: &proof,
            target: &target,
            delivery: MessageDelivery::Queue,
            text: "This unloaded queue request must be rejected without resuming.",
        })
        .await,
        "threadNotLoaded",
    )?;
    if is_loaded(&mut proof, &thread_id).await? {
        return Err("Rejected explicit queue implicitly loaded its target".into());
    }
    proof.record(
        "unloadedQueueRejectedWithoutResume",
        json!({"target":target}),
    )?;
    proof.client.close().await?;
    Ok(())
}

struct MessageProbeInput<'a> {
    proof: &'a ProofContext,
    target: &'a SessionRef,
    delivery: MessageDelivery,
    text: &'a str,
}

async fn send_agent_probe(input: MessageProbeInput<'_>) -> Result<DeliveryReceipt, ClientError> {
    // Server rejections retire a Control connection; each independent send has its own.
    let mut client =
        ControlClient::connect(&input.proof.service_directory, "busy-proof", "1").await?;
    let result = client
        .send_agent_message(SessionMessageSendParams {
            target: input.target.clone(),
            generation_guard: Some(input.proof.generation.clone()),
            message: MessageContent::Agent {
                sender: input.target.clone(),
                text: input
                    .text
                    .to_owned()
                    .try_into()
                    .map_err(|_| ClientError::Protocol("invalid proof text"))?,
            },
            mode: input.delivery,
            correlation: None,
        })
        .await;
    let _closed = client.close().await;
    result
}

fn require_rejection(result: Result<DeliveryReceipt, ClientError>, kind: &str) -> ProofResult<()> {
    match result {
        Ok(receipt)
            if matches!(&receipt.outcome,
                collaboration_client::protocol::DeliveryOutcome::NotSubmitted { reason, .. }
                if reason == kind)
                && receipt.client.is_none() =>
        {
            Ok(())
        }
        result => {
            Err(format!("Expected {kind} without a client receipt; observed {result:?}").into())
        }
    }
}

async fn is_loaded(proof: &mut ProofContext, thread_id: &str) -> ProofResult<bool> {
    let response = proof
        .native
        .request_validated(
            &proof.schemas,
            NativeOperation::ListLoadedThreads,
            json!({}),
        )
        .await?;
    if response
        .get("nextCursor")
        .is_some_and(|cursor| !cursor.is_null())
    {
        return Err("Unexpected pagination in isolated loaded-thread inventory".into());
    }
    let threads = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Loaded-thread inventory missing")?;
    Ok(threads.iter().any(|id| id.as_str() == Some(thread_id)))
}
