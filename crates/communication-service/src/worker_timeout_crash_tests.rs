//! Owned worker process exits at timeout boundaries; recovery observes without repeating interruption.
use super::*;
use agent_automation::{
    ContinuityInput, InstructionText, NativeEffectEvidence, OperationId, ScheduleDefinition,
    TimingRule,
};
use automation_storage::{
    RunDispatchIntent, RunSubmissionOutcome, RunSubmissionResult, ScheduleCreate,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_tungstenite::tungstenite::Message;
const ROOT_ENV: &str = "WORKER_TIMEOUT_TEST_ROOT";
const STAGE_ENV: &str = "WORKER_TIMEOUT_TEST_STAGE";
const EXIT_CODE: i32 = 96;
const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
type TestResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

fn child_root() -> TestResult<PathBuf> {
    let root = PathBuf::from(std::env::var(ROOT_ENV)?);
    if root.parent() != Some(Path::new("/tmp"))
        || !root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("worker-stop-"))
    {
        return Err("refusing non-fixture worker root".into());
    }
    Ok(root)
}
pub(super) fn checkpoint(stage: &str) {
    if std::env::var(STAGE_ENV).as_deref() == Ok(stage) && child_root().is_ok() {
        std::process::exit(EXIT_CODE);
    }
}
#[tokio::test]
async fn worker_timeout_crash_preserves_occupancy_until_exact_cessation() -> TestResult<()> {
    for stage in ["stop-intent", "interrupt-response"] {
        let root =
            PathBuf::from("/tmp").join(format!("worker-stop-{}", OperationId::generate().as_str()));
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "scheduled_run_worker::timeout_crash_tests::worker_timeout_crash_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env(ROOT_ENV, &root)
                .env(STAGE_ENV, stage)
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        if output.status.code() != Some(EXIT_CODE) {
            return Err(format!(
                "{stage}: expected checkpoint exit, got {:?}; {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        if root.join("interrupt-witness.json").exists() != (stage == "interrupt-response") {
            return Err("interrupt witness disagrees with process-loss boundary".into());
        }
        recover(&root).await?;
        for entry in std::fs::read_dir(&root)? {
            std::fs::remove_file(entry?.path())?;
        }
        std::fs::remove_dir(root)?;
    }
    Ok(())
}
#[tokio::test]
#[ignore = "owned child exits at the selected worker timeout boundary"]
async fn worker_timeout_crash_child() -> TestResult<()> {
    let root = child_root()?;
    let mut store = AutomationStore::open(&root.join("automation.sqlite")).await?;
    let endpoint: EndpointRef =
        serde_json::from_value(json!({"serviceId":SERVICE_ID,"endpointId":"codex-local"}))?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":SERVICE_ID,"generation":1}))?;
    let target = SessionRef {
        endpoint: endpoint.clone(),
        session_id: "recorded-thread".to_owned().try_into()?,
    };
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Inspect task".to_owned())?,
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
                destination: ExecutionDestination::OwnedThread {
                    target: target.clone(),
                    cwd: root.to_string_lossy().into_owned(),
                },
                execution_timeout_seconds: Some(1),
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let run_id = store
        .enqueue_due_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60000)
        .await?
        .ok_or("run missing")?;
    store
        .admit_waiting_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60001)
        .await?;
    let mut effects: NativeEffectEvidence<SessionRef, CodexGeneration> = serde_json::from_value(
        json!({
            "target":target,"generation":generation,"clientUserMessageId":run_id,"nativeTurnId":null,"nativeSubmissionId":null,
            "allocation":"notRequested","resume":"notRequested","submission":"dispatching","cessation":"unconfirmed"
        }),
    )?;
    store
        .begin_run_dispatch::<_, EndpointRef, _, NativeSendReceipt>(RunDispatchIntent {
            run_id: run_id.clone(),
            effects: effects.clone(),
            configured_timeout_seconds: 1,
            now_ms: 61000,
        })
        .await?;
    effects.native_turn_id = Some("recorded-turn".into());
    effects.submission = agent_automation::SubmissionEffect::Accepted;
    let receipt: NativeSendReceipt = serde_json::from_value(
        json!({"target":target,"generation":generation,"inputKind":"agent","representation":"declaredAgentText","clientUserMessageId":run_id,"resumeEffect":"notRequested","acceptance":{"kind":"nativeInputAccepted","operation":"turnStart","disposition":"startedOrSteered","turnId":"recorded-turn"}}),
    )?;
    store
        .record_run_submission::<_, EndpointRef, _, _>(RunSubmissionResult {
            run_id: run_id.clone(),
            effects,
            outcome: RunSubmissionOutcome::Accepted {
                turn_id: "recorded-turn".into(),
                receipt,
            },
        })
        .await?;
    std::fs::write(root.join("run-id.json"), serde_json::to_vec(&run_id)?)?;
    let socket = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let witness = root.join("interrupt-witness.json");
    let _backend = tokio::spawn(async move {
        for method in ["thread/read", "turn/interrupt"] {
            let (stream, _) = listener.accept().await?;
            let mut stream = tokio_tungstenite::accept_async(stream).await?;
            let request = request_after_initialize(&mut stream).await?;
            if request.get("method").and_then(Value::as_str) != Some(method) {
                return Err("unexpected native method".into());
            }
            let result = if method == "thread/read" {
                history("inProgress")
            } else {
                if request.pointer("/params/threadId").and_then(Value::as_str)
                    != Some("recorded-thread")
                    || request.pointer("/params/turnId").and_then(Value::as_str)
                        != Some("recorded-turn")
                {
                    return Err("wrong interrupt identity".into());
                }
                std::fs::write(&witness, serde_json::to_vec(&request)?)?;
                json!({})
            };
            stream
                .send(Message::Text(
                    json!({"id":request.get("id"),"result":result})
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let worker = fixture_worker(
        Arc::new(Mutex::new(store)),
        &root,
        WorkerEndpoint { socket, number: 1 },
    )?;
    worker.step(run_id).await?;
    Err("worker did not reach crash checkpoint".into())
}
async fn recover(root: &Path) -> TestResult<()> {
    let run_id: RunId = serde_json::from_slice(&std::fs::read(root.join("run-id.json"))?)?;
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let before = store
        .lock()
        .await
        .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&run_id)
        .await?;
    if before.phase != RunPhase::Stopping
        || before.evidence.native.cessation != agent_automation::CessationEvidence::Unconfirmed
    {
        return Err("crash invented cessation or lost stopping intent".into());
    }
    let timing = serde_json::to_value(&before.evidence.timing)?;
    let socket = root.join("recovered.sock");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let backend = tokio::spawn(async move {
        for status in ["inProgress", "interrupted"] {
            let (stream, _) = listener.accept().await?;
            let mut stream = tokio_tungstenite::accept_async(stream).await?;
            let request = request_after_initialize(&mut stream).await?;
            if request.get("method").and_then(Value::as_str) != Some("thread/read")
                || request.pointer("/params/threadId").and_then(Value::as_str)
                    != Some("recorded-thread")
            {
                return Err("recovery resent interrupt or read another thread".into());
            }
            stream
                .send(Message::Text(
                    json!({"id":request.get("id"),"result":history(status)})
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        if tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_ok()
        {
            return Err("recovery made an extra native call".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let worker = fixture_worker(store.clone(), root, WorkerEndpoint { socket, number: 2 })?;
    for stopped in [false, true] {
        worker.step(run_id.clone()).await?;
        let record = store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&run_id)
            .await?;
        if serde_json::to_value(&record.evidence.timing)? != timing
            || record.native_turn_id.as_deref() != Some("recorded-turn")
        {
            return Err("recovery reset timeout or changed turn identity".into());
        }
        let inventory = store
            .lock()
            .await
            .inspect_schedule::<SessionRef, EndpointRef>(&record.schedule_id)
            .await?;
        if stopped {
            if record.phase != RunPhase::Finished
                || record.evidence.native.cessation
                    != agent_automation::CessationEvidence::Confirmed
                || inventory.active_run_id.is_some()
            {
                return Err("exact cessation did not release occupancy".into());
            }
        } else if record.phase != RunPhase::Stopping
            || inventory.active_run_id.as_ref() != Some(&run_id)
        {
            return Err("unrelated completion or acknowledgment released occupancy".into());
        }
    }
    tokio::time::timeout(Duration::from_secs(3), backend).await???;
    drop(worker);
    drop(store);
    Ok(())
}
fn history(status: &str) -> Value {
    json!({"thread":{"id":"recorded-thread","turns":[{"id":"unrelated-turn","status":"completed","items":[]},{"id":"recorded-turn","status":status,"items":[]}]}})
}
async fn request_after_initialize(
    stream: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> TestResult<Value> {
    let init: Value = serde_json::from_str(
        stream
            .next()
            .await
            .ok_or("missing initialize")??
            .to_text()?,
    )?;
    stream
        .send(Message::Text(
            json!({"id":init.get("id"),"result":{}}).to_string().into(),
        ))
        .await?;
    let _initialized = stream.next().await.ok_or("missing initialized")??;
    Ok(serde_json::from_str(
        stream.next().await.ok_or("missing request")??.to_text()?,
    )?)
}
struct WorkerEndpoint {
    socket: PathBuf,
    number: u64,
}
fn fixture_worker(
    store: Arc<Mutex<AutomationStore>>,
    root: &Path,
    endpoint: WorkerEndpoint,
) -> TestResult<ScheduledRunWorker> {
    let WorkerEndpoint { socket, number } = endpoint;
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".into(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
        &bundle,
    )?);
    let generation = serde_json::from_value(
        json!({"serviceEpoch":if number == 1 { SERVICE_ID } else { "00000000-0000-4000-8000-000000000003" },"generation":number}),
    )?;
    let gate = crate::NativeGenerationGate::default();
    gate.activate(generation, socket, Some(schemas))?;
    Ok(ScheduledRunWorker {
        store,
        backend: Some(NativeControlBackend {
            endpoint: serde_json::from_value(
                json!({"serviceId":SERVICE_ID,"endpointId":"codex-local"}),
            )?,
            gate,
            codex_home: root.to_owned(),
        }),
        configuration: crate::AutomationConfigurationHandle::default(),
    })
}
