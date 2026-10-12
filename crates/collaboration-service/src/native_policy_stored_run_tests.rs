//! Scheduled policy crosses the real stored evidence sink and AutomationStore.
use super::*;
use agent_automation::{
    ContinuityInput, InstructionText, PreparationEffect, RunPhase, ScheduleDefinition, ScheduleId,
    SubmissionEffect, TimingRule,
};
use automation_storage::{AutomationStore, RunPreparationIntent, ScheduleCreate};
use collaboration_protocol::{EndpointRef, OperationId};

type NativeRunRecord = agent_automation::RunRecord<
    SessionRef,
    EndpointRef,
    CodexGeneration,
    crate::stored_run_receipt::StoredRunReceipt,
>;
struct StoredPolicyRun {
    store: Arc<tokio::sync::Mutex<AutomationStore>>,
    run_id: RunId,
    schedule_id: ScheduleId,
    initial: RouteEffectEvidence<SessionRef, CodexGeneration>,
}
impl StoredPolicyRun {
    async fn create(fixture: &RecordedPolicyFixture) -> Result<Self, TestError> {
        let mut store =
            AutomationStore::open(&fixture.directory.path().join("automation.sqlite")).await?;
        let instruction = store
            .create_instruction(
                &OperationId::generate(),
                &InstructionText::try_from("Verify policy".to_owned())?,
                0,
            )
            .await?;
        let schedule = store
            .create_schedule(&ScheduleCreate::<SessionRef, EndpointRef> {
                operation_id: OperationId::generate(),
                definition: ScheduleDefinition {
                    instruction_id: instruction.instruction_id,
                    timing: TimingRule::Interval { seconds: 60 },
                    enabled: true,
                    destination: agent_automation::ExecutionDestination::OwnedThread {
                        target: target(),
                        cwd: fixture.directory.path().to_string_lossy().into_owned(),
                    },
                    execution_timeout_seconds: Some(120),
                    model: Some("fixture-model".into()),
                    effort: Some("medium".into()),
                },
                imported_continuity: ContinuityInput::None,
                now_ms: 0,
            })
            .await?;
        let run_id = store
            .enqueue_due_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60000)
            .await?
            .ok_or("run not enqueued")?;
        store
            .admit_waiting_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60001)
            .await?;
        let initial = fixture.scheduled_run().recorded;
        store.begin_run_preparation::<SessionRef,EndpointRef,CodexGeneration,crate::stored_run_receipt::StoredRunReceipt>(RunPreparationIntent{run_id:run_id.clone(),effects:initial.clone()}).await?;
        Ok(Self {
            store: Arc::new(tokio::sync::Mutex::new(store)),
            run_id,
            schedule_id: schedule.schedule_id,
            initial,
        })
    }
    async fn read(&self) -> Result<NativeRunRecord, TestError> {
        Ok(self.store.lock().await.read_run(&self.run_id).await?)
    }
    fn sink(
        &self,
        initial: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> crate::scheduled_run_evidence_sink::StoredRunEvidenceSink {
        crate::scheduled_run_evidence_sink::StoredRunEvidenceSink::new(
            self.store.clone(),
            crate::AutomationConfigurationHandle::default(),
            self.run_id.clone(),
            self.schedule_id.clone(),
            Some(initial),
        )
    }
    async fn submission(
        &self,
        fixture: &RecordedPolicyFixture,
        recorded: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> Result<crate::ScheduledRunSubmission, TestError> {
        let mut submission = fixture.scheduled_run();
        submission.run_id = self.run_id.clone();
        submission.inputs = self.read().await?.inputs.ok_or("captured inputs missing")?;
        submission.recorded = recorded;
        Ok(submission)
    }
}

async fn stored_two_attempts(
    invalid_second: bool,
) -> Result<(NativeRunRecord, RunSubmission, Vec<Vec<Value>>), TestError> {
    let fixture = RecordedPolicyFixture::create().await?;
    let stored = StoredPolicyRun::create(&fixture).await?;
    let server = policy_server(
        &fixture,
        PolicyServerCase {
            cold: ReplyPolicy::Matching,
            loaded: ReplyPolicy::Matching,
            loaded_delivery: MessageDelivery::Auto,
            turn_intent: None,
            retire_loaded: None,
            stored_run: Some((stored.store.clone(), stored.run_id.clone())),
        },
    )
    .await?;
    let directory = crate::EndpointDirectory::new(fixture.backend.endpoint.service_id.clone());
    directory.publish(fixture.endpoint.clone())?;
    let route = crate::CodexAppServerDeliveryRoute::new(
        fixture.backend.endpoint.service_id.clone(),
        directory,
        fixture.backend.clone(),
        Arc::new(crate::UnmaterializedThreadHolder::new()),
    )
    .with_approval_broker(&fixture.broker);
    let scheduled = route.scheduled_runs().ok_or("scheduled route missing")?;
    let first_sink = stored.sink(stored.initial.clone());
    let first = tokio::time::timeout(
        Duration::from_secs(5),
        scheduled.submit_run(
            stored.submission(&fixture, stored.initial.clone()).await?,
            &first_sink,
        ),
    )
    .await??;
    if !matches!(first, RunSubmission::NotStartedBusy) {
        return Err("cold accepted attachment must remain busy".into());
    }
    let first_record = stored.read().await?;
    if first_record.phase != RunPhase::Preparing
        || first_record.evidence.timing.is_some()
        || !matches!(&first_record.evidence.route,Some(RouteEffectEvidence::CodexAppServer(effects)) if effects.resume==PreparationEffect::Accepted && effects.submission==SubmissionEffect::NotDispatched)
    {
        return Err(
            "first attempt lost persisted accepted preparation or acquired execution timing".into(),
        );
    }
    if invalid_second {
        fixture
            .broker
            .register_route(ApprovalRoute {
                thread_id: THREAD.into(),
                created_by: target(),
                approver: target(),
                access: collaboration_protocol::RouterAccess::WorkspaceWrite,
                scratch_path: "relative-invalid-scratch".into(),
                root_message_id: None,
            })
            .await?;
    }
    let recorded = first_record
        .evidence
        .route
        .ok_or("first stored route missing")?;
    let second_sink = stored.sink(recorded.clone());
    let second = tokio::time::timeout(
        Duration::from_secs(5),
        scheduled.submit_run(stored.submission(&fixture, recorded).await?, &second_sink),
    )
    .await??;
    let requests = tokio::time::timeout(Duration::from_secs(5), server).await???;
    // Fresh owning store decodes the actual committed transition, not sink.latest().
    let mut reopened =
        AutomationStore::open(&fixture.directory.path().join("automation.sqlite")).await?;
    let record=reopened.read_run::<SessionRef,EndpointRef,CodexGeneration,crate::stored_run_receipt::StoredRunReceipt>(&stored.run_id).await?;
    reopened.close().await?;
    Ok((record, second, requests))
}

#[tokio::test]
async fn policy_stored_accepted_resume_survives_second_preparation_failure_causal() {
    let (record, result, requests) = stored_two_attempts(true).await.unwrap();
    assert!(matches!(result, RunSubmission::Rejected(_)));
    let Some(RouteEffectEvidence::CodexAppServer(effects)) = record.evidence.route else {
        panic!("native stored effects missing");
    };
    assert_eq!(
        effects.resume,
        PreparationEffect::Accepted,
        "attempt two must preserve attempt one's accepted resume"
    );
    assert_eq!(effects.submission, SubmissionEffect::NotDispatched);
    assert_eq!(record.phase, RunPhase::Preparing);
    assert!(record.evidence.timing.is_none());
    assert!(requests[1].iter().all(|request| !matches!(
        request["method"].as_str(),
        Some("thread/resume" | "turn/start")
    )));
}

#[tokio::test]
async fn policy_stored_matching_loaded_attempt_records_real_intent_and_timing() {
    let (record, result, requests) = stored_two_attempts(false).await.unwrap();
    assert!(matches!(result, RunSubmission::Started(_)));
    let timing = record.evidence.timing.unwrap();
    assert_eq!(timing.effective_timeout_seconds, 120);
    let Some(RouteEffectEvidence::CodexAppServer(effects)) = record.evidence.route else {
        panic!("native stored effects missing");
    };
    assert_eq!(effects.resume, PreparationEffect::Accepted);
    assert_eq!(effects.submission, SubmissionEffect::Dispatching);
    assert!(
        requests[0]
            .iter()
            .all(|request| request["method"] != "turn/start")
    );
    assert_eq!(
        requests[1]
            .iter()
            .find(|request| request["method"] == "thread/resume")
            .unwrap()["params"],
        json!({"threadId":THREAD,"excludeTurns":true})
    );
    assert!(
        requests[1]
            .iter()
            .any(|request| request["method"] == "turn/start")
    );
}
