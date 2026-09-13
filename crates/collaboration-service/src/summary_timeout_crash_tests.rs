//! Actual child exits at timeout intent/receipt boundaries; recovery observes without resend.
use super::*;
use std::path::{Path, PathBuf};
const ROOT_ENV: &str = "SUMMARY_TIMEOUT_TEST_ROOT";
const STAGE_ENV: &str = "SUMMARY_TIMEOUT_TEST_STAGE";
const CRASH_EXIT: i32 = 95;

pub(super) fn child_root() -> TestResult<Option<PathBuf>> {
    let Some(root) = std::env::var_os(ROOT_ENV) else {
        return Ok(None);
    };
    let root = PathBuf::from(root);
    if root.parent() != Some(Path::new("/tmp"))
        || !root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("summary-crash-"))
    {
        return Err("refusing non-fixture timeout root".into());
    }
    Ok(Some(root))
}
pub(super) fn checkpoint(stage: &str) {
    if std::env::var(STAGE_ENV).as_deref() == Ok(stage)
        && child_root().is_ok_and(|root| root.is_some())
    {
        std::process::exit(CRASH_EXIT);
    }
}
#[tokio::test]
#[ignore = "owned child exits at a parent-selected timeout checkpoint"]
async fn summary_timeout_crash_child() -> TestResult<()> {
    if child_root()?.is_none() {
        return Err("missing owned fixture root".into());
    }
    exercise_timeout_connection(false).await
}
#[tokio::test]
async fn summary_timeout_crash_recovery_never_replays_uncertain_interrupt() -> TestResult<()> {
    for stage in ["stop-intent", "interrupt-response"] {
        let root = PathBuf::from("/tmp").join(format!(
            "summary-crash-{}",
            agent_automation::OperationId::generate().as_str()
        ));
        let child = tokio::time::timeout(Duration::from_secs(15), tokio::process::Command::new(std::env::current_exe()?)
            .args(["--exact", "summary_native_worker::summary_deadline_tests::crash_tests::summary_timeout_crash_child", "--ignored", "--nocapture"])
            .env(ROOT_ENV, &root).env(STAGE_ENV, stage).kill_on_drop(true).output()).await??;
        if child.status.code() != Some(CRASH_EXIT) {
            return Err(format!(
                "{stage}: child did not reach checkpoint: {:?}; {}",
                child.status.code(),
                String::from_utf8_lossy(&child.stderr)
            )
            .into());
        }
        if root.join("interrupt-request.json").exists() != (stage == "interrupt-response") {
            return Err("native request witness disagrees with crash boundary".into());
        }
        recover_and_observe(&root).await?;
        for entry in std::fs::read_dir(&root)? {
            std::fs::remove_file(entry?.path())?;
        }
        std::fs::remove_dir(root)?;
    }
    Ok(())
}
async fn recover_and_observe(root: &Path) -> TestResult<()> {
    let id: agent_automation::RunId =
        serde_json::from_slice(&std::fs::read(root.join("run-id.json"))?)?;
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let before = store
        .lock()
        .await
        .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&id)
        .await?;
    let attempt = before
        .summary_attempt
        .as_ref()
        .ok_or("summary attempt missing")?;
    if attempt.phase != SummaryPhase::Stopping
        || attempt.effects.cessation != CessationEvidence::Unconfirmed
        || attempt.deadline_at_ms != 1000
    {
        return Err("crash lost stopping intent, original deadline or uncertainty".into());
    }
    let old_attempt_id = attempt.attempt_id.clone();
    let socket_path = root.join("recovered-native.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".into(),
        std::fs::read(root.join("schema.json"))?,
    )]))?;
    let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
        &bundle,
    )?);
    let generation: CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000003","generation":2}),
    )?;
    let gate = crate::NativeGenerationGate::default();
    gate.activate(generation, socket_path, Some(schemas))?;
    let admission = gate.acquire()?;
    let backend = tokio::spawn(async move {
        for status in ["inProgress", "interrupted"] {
            let (socket, _) = listener.accept().await?;
            let mut socket = tokio_tungstenite::accept_async(socket).await?;
            let init: Value =
                serde_json::from_str(socket.next().await.ok_or("missing init")??.to_text()?)?;
            socket
                .send(Message::Text(
                    json!({"id":init.get("id"),"result":{}}).to_string().into(),
                ))
                .await?;
            let _initialized = socket.next().await.ok_or("missing initialized")??;
            let request: Value =
                serde_json::from_str(socket.next().await.ok_or("missing read")??.to_text()?)?;
            if request.get("method").and_then(Value::as_str) != Some("thread/read")
                || request.pointer("/params/threadId").and_then(Value::as_str)
                    != Some("summary-thread")
            {
                return Err("recovery resent interrupt or observed wrong thread".into());
            }
            socket.send(Message::Text(json!({"id":request.get("id"),"result":{"thread":{"id":"summary-thread","turns":[{"id":"unrelated-turn","status":"completed","items":[]},{"id":"summary-turn","status":status,"items":[]}]}}}).to_string().into())).await?;
        }
        if tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_ok()
        {
            return Err("recovery made an extra native call".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    for confirmed in [false, true] {
        let record = store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&id)
            .await?;
        step(SummaryStep {
            work: SummaryWork::Advance,
            store: &store,
            admission: &admission,
            record,
            timeout_seconds: 999,
        })
        .await?;
        let observed = store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&id)
            .await?;
        let summary = observed.summary_attempt.ok_or("summary disappeared")?;
        if summary.attempt_id != old_attempt_id
            || summary.deadline_at_ms != 1000
            || summary.effective_timeout_seconds != 1
        {
            return Err("recovery changed attempt identity or granted a new budget".into());
        }
        if confirmed {
            if summary.phase != SummaryPhase::Failed
                || summary.effects.cessation != CessationEvidence::Confirmed
            {
                return Err("exact interrupted turn did not establish cessation".into());
            }
        } else if summary.phase != SummaryPhase::Stopping
            || summary.effects.cessation != CessationEvidence::Unconfirmed
        {
            return Err(
                "unrelated completion or prior acknowledgement fabricated cessation".into(),
            );
        }
    }
    tokio::time::timeout(Duration::from_secs(3), backend).await???;
    drop(store);
    Ok(())
}
