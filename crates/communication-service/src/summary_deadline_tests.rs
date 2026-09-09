//! An unavailable transcript does not erase a known summary's captured deadline.
use super::*;
use futures_util::{SinkExt, StreamExt};
use sqlx::Connection;
use std::{collections::BTreeMap, time::Duration};
use tokio_tungstenite::tungstenite::Message;
type TestResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn expired_summary_requests_interrupt_when_history_is_rejected() -> TestResult<()> {
    let directory = std::path::PathBuf::from("/tmp").join(format!(
        "summary-deadline-{}",
        agent_automation::OperationId::generate().as_str()
    ));
    std::fs::create_dir(&directory)?;
    let path = directory.join("automation.sqlite");
    let store = AutomationStore::open(&path).await?;
    let endpoint: EndpointRef = serde_json::from_value(
        json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"}),
    )?;
    let target = SessionRef {
        endpoint: endpoint.clone(),
        session_id: "summary-thread".to_owned().try_into()?,
    };
    let generation: CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1}),
    )?;
    let run_id = agent_automation::RunId::generate();
    let schedule_id = agent_automation::ScheduleId::generate();
    let instruction_id = agent_automation::InstructionId::generate();
    let revision_id = agent_automation::RevisionId::generate();
    let effects = agent_automation::NativeEffectEvidence {
        target: Some(target.clone()),
        generation: Some(generation.clone()),
        client_user_message_id: None,
        native_turn_id: Some("summary-turn".into()),
        native_submission_id: None,
        allocation: PreparationEffect::Accepted,
        resume: PreparationEffect::NotRequested,
        submission: SubmissionEffect::Accepted,
        cessation: CessationEvidence::Unconfirmed,
    };
    let attempt = SummaryAttempt {
        attempt_id: agent_automation::AttemptId::generate(),
        source_target: target.clone(),
        source_turn_id: "worker-turn".into(),
        target: Some(target),
        native_turn_id: Some("summary-turn".into()),
        effective_timeout_seconds: 1,
        started_at_ms: 0,
        deadline_at_ms: 1000,
        phase: SummaryPhase::Running,
        effects: effects.clone(),
        explanation: None,
    };
    let mut seed = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(true),
    )
    .await?;
    let mut transaction = seed.begin().await?;
    sqlx::query("INSERT INTO instruction_documents VALUES (?,?, 'fixture',0)")
        .bind(instruction_id.as_str())
        .bind(revision_id.as_str())
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO instruction_revisions VALUES (?,?,'fixture',NULL,0)")
        .bind(revision_id.as_str())
        .bind(instruction_id.as_str())
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO schedule_definitions VALUES (?,?,?,0,'{}','{}',0,0)")
        .bind(schedule_id.as_str())
        .bind(agent_automation::ChangeId::generate().as_str())
        .bind(instruction_id.as_str())
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO workflow_runs(run_id,schedule_id,due_at_ms,run_status,execution_evidence_json,summary_attempt_json) VALUES (?,?,0,'summaryRunning','{}',?)").bind(run_id.as_str()).bind(schedule_id.as_str()).bind(serde_json::to_string(&attempt)?).execute(&mut *transaction).await?;
    transaction.commit().await?;
    seed.close().await?;
    let record = RunRecord {
        run_id: run_id.clone(),
        schedule_id,
        due_at_ms: 0,
        phase: RunPhase::SummaryRunning,
        inputs: None,
        thread_binding_id: None,
        native_turn_id: Some("worker-turn".into()),
        evidence: agent_automation::RunExecutionEvidence {
            native: effects,
            timing: None,
            acceptance: None,
        },
        worker_outcome: None,
        summary_attempt: Some(attempt),
        summary_text: None,
        summary_source: None,
        completed_at_ms: None,
    };
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
    let socket_path = directory.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let gate = crate::NativeGenerationGate::default();
    gate.activate(generation, socket_path, Some(schemas))?;
    let admission = gate.acquire()?;
    let server = tokio::spawn(async move {
        for method in ["thread/read", "turn/interrupt"] {
            let (socket, _) = listener.accept().await?;
            let mut socket = tokio_tungstenite::accept_async(socket).await?;
            let init: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .ok_or("missing initialize")??
                    .to_text()?,
            )?;
            socket
                .send(Message::Text(
                    json!({"id":init["id"],"result":{}}).to_string().into(),
                ))
                .await?;
            let _notification = socket.next().await.ok_or("missing initialized")??;
            let request: Value =
                serde_json::from_str(socket.next().await.ok_or("missing request")??.to_text()?)?;
            if request["method"] != method {
                return Err("unexpected native operation".into());
            }
            let response = if method == "thread/read" {
                json!({"id":request["id"],"error":{"code":-32602,"message":"history unavailable"}})
            } else {
                if request.pointer("/params/threadId") != Some(&json!("summary-thread"))
                    || request.pointer("/params/turnId") != Some(&json!("summary-turn"))
                {
                    return Err("interrupted wrong summary turn".into());
                }
                json!({"id":request["id"],"result":{}})
            };
            socket
                .send(Message::Text(response.to_string().into()))
                .await?;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let store = Arc::new(Mutex::new(store));
    step(SummaryStep {
        work: SummaryWork::Advance,
        store: &store,
        admission: &admission,
        record,
        timeout_seconds: 1,
    })
    .await?;
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    Ok(())
}
