//! Persisted broker and socket fixture for native loaded-policy scenarios.
use super::*;

pub(super) struct RecordedPolicyFixture {
    pub(super) directory: tempfile::TempDir,
    pub(super) backend: NativeControlBackend,
    pub(super) generation: CodexGeneration,
    pub(super) endpoint: EndpointDescription,
    pub(super) broker: Arc<ServiceInteractionBroker>,
    pub(super) filesystem: serde_json::Map<String, Value>,
}

impl RecordedPolicyFixture {
    pub(super) async fn create() -> Result<Self, TestError> {
        Self::create_with_authority(true).await
    }
    pub(super) async fn create_without_route() -> Result<Self, TestError> {
        Self::create_with_authority(false).await
    }
    async fn create_with_authority(register_route: bool) -> Result<Self, TestError> {
        let directory = tempfile::tempdir()?;
        let (backend, generation, endpoint) = backend(directory.path());
        let scratch = directory.path().join("scratch");
        tokio::fs::create_dir(&scratch).await?;
        let broker = ServiceInteractionBroker::load(
            SERVICE.to_owned().try_into()?,
            backend.clone(),
            directory.path().join("approval-routes.json"),
        )
        .await?;
        if register_route {
            broker
                .register_route(ApprovalRoute {
                    thread_id: THREAD.into(),
                    created_by: target(),
                    approver: target(),
                    access: collaboration_protocol::RouterAccess::WorkspaceWrite,
                    scratch_path: scratch.to_string_lossy().into_owned(),
                    root_message_id: None,
                })
                .await?;
        }
        drop(broker);
        let broker = ServiceInteractionBroker::load(
            SERVICE.to_owned().try_into()?,
            backend.clone(),
            directory.path().join("approval-routes.json"),
        )
        .await?;
        Ok(Self {
            filesystem: expected_filesystem(&scratch)?,
            directory,
            backend,
            generation,
            endpoint,
            broker,
        })
    }

    pub(super) fn message_request<'a>(
        &'a self,
        broker: &'a std::sync::Weak<dyn ApprovalBroker>,
    ) -> NativeMessageRequest<'a> {
        NativeMessageRequest {
            params: NativeMessageParams {
                target: target(),
                generation: self.generation.clone(),
                body: NativeMessageBody::PreparedPush(
                    "validate loaded policy".to_owned().try_into().unwrap(),
                ),
                delivery: MessageDelivery::Auto,
                client_user_message_id: Some("loaded-policy-input".to_owned().try_into().unwrap()),
            },
            id: json!("loaded-policy-input"),
            service_id: &self.backend.endpoint.service_id,
            backend: &self.backend,
            endpoints: std::slice::from_ref(&self.endpoint),
            held_connection: None,
            load_policy: LoadPolicy::MayLoad,
            approval_broker: Some(broker),
            recorded_route: None,
            attempt_deadline: None,
        }
    }

    pub(super) fn scheduled_run(&self) -> crate::ScheduledRunSubmission {
        crate::ScheduledRunSubmission {
            run_id: RunId::generate(), target: target(), payload: crate::scheduled_run_contract::ScheduledRunPayload::Fresh { task_input: "validate scheduled policy".to_owned().try_into().unwrap() }, precondition: crate::DeliveryPrecondition::Unpinned,
            inputs: serde_json::from_value::<CapturedRunInputs<_, _>>(json!({ "scheduleChangeId":agent_automation::ChangeId::generate(),"instructionRevisionId":agent_automation::RevisionId::generate(),"instructionText":"Validate policy","continuity":{"kind":"none"},"executionConfiguration":{"destination":{"kind":"ownedThread","target":target(),"cwd":self.directory.path()},"executionTimeoutSeconds":120,"model":"fixture-model","effort":"medium"} })).unwrap(),
            recorded: serde_json::from_value(json!({"kind":"codexAppServer","target":target(),"generation":self.generation,"clientUserMessageId":"loaded-policy-input","nativeTurnId":null,"nativeSubmissionId":null,"allocation":"notRequested","resume":"notRequested","submission":"notDispatched","cessation":"notApplicable"})).unwrap(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReplyPolicy {
    Matching,
    Narrowed,
    MissingSettings,
}
pub(super) struct PolicyServerCase {
    pub(super) cold: ReplyPolicy,
    pub(super) loaded: ReplyPolicy,
    pub(super) loaded_delivery: MessageDelivery,
    pub(super) turn_intent: Option<Arc<AtomicBool>>,
    pub(super) retire_loaded: Option<NativeGenerationGate>,
    pub(super) stored_run: Option<(
        Arc<tokio::sync::Mutex<automation_storage::AutomationStore>>,
        RunId,
    )>,
}

/// Native replies and synthetic schema/endpoint metadata stand in for upstream; storage and sockets are real.
/// CapturingRunSink below is a separate stand-in; stored-run cases use the real sink.
pub(super) async fn policy_server(
    fixture: &RecordedPolicyFixture,
    scenario: PolicyServerCase,
) -> Result<tokio::task::JoinHandle<Result<Vec<Vec<Value>>, TestError>>, TestError> {
    let listener = tokio::net::UnixListener::bind(fixture.directory.path().join("native.sock"))?;
    let cwd = fixture.directory.path().to_string_lossy().into_owned();
    let roots = fixture
        .filesystem
        .iter()
        .filter(|(_, access)| *access == &json!("write"))
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    Ok(tokio::spawn(async move {
        let mut exchanges = Vec::new();
        for request_index in 0..2 {
            let (stream, _) = listener.accept().await?;
            let mut socket = tokio_tungstenite::accept_async(stream).await?;
            let mut requests = Vec::new();
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let request: Value = serde_json::from_str(&text)?;
                let method = request["method"].as_str().ok_or("method missing")?;
                let result = match method {
                    "initialize" => json!({}),
                    "initialized" => continue,
                    "thread/read" => {
                        json!({"thread":{"id":THREAD,"cwd":cwd,"status":{"type":if request_index==0 {"notLoaded"} else if scenario.loaded_delivery==MessageDelivery::Steer {"active"} else {"idle"}}}})
                    }
                    "thread/resume" => {
                        let policy = if request_index == 0 {
                            scenario.cold
                        } else {
                            scenario.loaded
                        };
                        let mut response = json!({"thread":{"id":THREAD,"cwd":cwd},"cwd":cwd,"activePermissionProfile":{"id":"router-workspace-write"},"sandbox":{"type":"workspaceWrite","networkAccess":true,"writableRoots":if matches!(policy, ReplyPolicy::Matching) {roots.clone()} else {vec![]}}});
                        if matches!(policy, ReplyPolicy::MissingSettings) {
                            response.as_object_mut().unwrap().remove("cwd");
                        }
                        response
                    }
                    "turn/start" => {
                        if let Some((store, run_id)) = &scenario.stored_run {
                            let record=store.lock().await.read_run::<SessionRef,collaboration_protocol::EndpointRef,CodexGeneration,crate::stored_run_receipt::StoredRunReceipt>(run_id).await?;
                            if record.evidence.timing.is_none()
                                || !matches!(record.evidence.route,Some(RouteEffectEvidence::CodexAppServer(effects)) if effects.submission==agent_automation::SubmissionEffect::Dispatching && effects.resume==agent_automation::PreparationEffect::Accepted)
                            {
                                return Err(
                                    "stored dispatch intent/timing missing before input".into()
                                );
                            }
                        }
                        if let Some(intent) = &scenario.turn_intent {
                            assert!(
                                intent.load(Ordering::SeqCst),
                                "scheduled intent must be recorded before input"
                            );
                        }
                        json!({"turn":{"id":"accepted-native-turn"}})
                    }
                    "thread/turns/list" => {
                        json!({"data":[{"id":"active-turn","status":"inProgress"}]})
                    }
                    "turn/steer" => json!({"turnId":"active-turn"}),
                    "thread/queue/add" => json!({"queuedSubmission":{"id":"queued-input"}}),
                    _ => return Err("unexpected native method".into()),
                };
                if method == "thread/resume"
                    && request_index == 1
                    && let Some(gate) = &scenario.retire_loaded
                {
                    gate.retire().unwrap();
                }
                requests.push(request.clone());
                socket
                    .send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .await?;
            }
            exchanges.push(requests);
        }
        Ok(exchanges)
    }))
}

/// Deterministic sink stand-in: no storage admission, persistence or real timing.
/// Stored-run tests use StoredRunEvidenceSink and AutomationStore instead.
pub(super) struct CapturingRunSink {
    pub(super) evidence: Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
    pub(super) turn_intent: Arc<AtomicBool>,
}
impl RunEvidenceSink for CapturingRunSink {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> crate::DeliveryFuture<'_, RunEvidenceDisposition> {
        if matches!(&evidence, RouteEffectEvidence::CodexAppServer(effects) if effects.submission == agent_automation::SubmissionEffect::Dispatching)
        {
            self.turn_intent.store(true, Ordering::SeqCst);
        }
        self.evidence.lock().unwrap().push(evidence);
        Box::pin(async {
            Ok(RunEvidenceDisposition::Recorded {
                timing: Some(ExecutionTiming {
                    dispatch_started_at_ms: 1_791_700_000_000,
                    deadline_at_ms: 1_791_700_120_000,
                    effective_timeout_seconds: 120,
                }),
            })
        })
    }
    fn record_stop_intent(&self) -> crate::DeliveryFuture<'_, RunEvidenceDisposition> {
        Box::pin(async { Ok(RunEvidenceDisposition::AdmissionRefused) })
    }
}
