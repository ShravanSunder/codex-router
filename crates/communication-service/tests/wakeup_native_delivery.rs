//! Real SQLite and WebSocket protocol fixture; this does not invoke Codex or a model.
use agent_automation::{ExpiryRule, TimingRule};
use automation_storage::{AutomationStore, WakeCreate};
use communication_protocol::{
    CodexGeneration, EndpointDescription, OperationId, SavedMessage, SessionRef,
};
use communication_service::{NativeControlBackend, NativeGenerationGate, ServiceIdentity};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sqlx::Connection;
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn timed_worker_uses_native_queue_and_persists_actual_receipt()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_delivery(NativeOutcome::Accepted).await
}
#[tokio::test]
async fn timed_worker_retains_lost_native_response_without_replay()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_delivery(NativeOutcome::ResponseLost).await
}
#[tokio::test]
async fn lost_queue_receipt_reconciles_exact_saved_input()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_delivery(NativeOutcome::ReconcileFound).await
}
#[tokio::test]
async fn queue_absence_never_proves_non_submission()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_delivery(NativeOutcome::ReconcileAbsent).await
}
#[tokio::test]
async fn matching_correlation_with_different_content_stays_uncertain()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_delivery(NativeOutcome::ReconcileMismatch).await
}
#[derive(Clone, Copy)]
enum NativeOutcome {
    Accepted,
    ResponseLost,
    ReconcileFound,
    ReconcileAbsent,
    ReconcileMismatch,
}
async fn exercise_delivery(
    outcome: NativeOutcome,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "wake-native-fixture-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let database = root.join("automation.sqlite");
    let store = Arc::new(tokio::sync::Mutex::new(
        AutomationStore::open(&database).await?,
    ));
    let path = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&path)?;
    let service_id = "00000000-0000-4000-8000-000000000001";
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let target: SessionRef = serde_json::from_value(
        json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"fixture-new-target"}),
    )?;
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
        "ThreadQueueList",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
        &bundle,
    )?);
    let description: EndpointDescription = serde_json::from_value(
        json!({"endpoint":target.endpoint,"label":"Fixture","availability":{"state":"available","observedAt":"2026-09-08T00:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]}),
    )?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation, path.clone(), Some(schemas))?;
    let identity = ServiceIdentity::new(
        service_id,
        service_id,
        &format!("sha256:{}", "a".repeat(64)),
    )?
    .with_endpoints(vec![description])?
    .with_automation_store(store.clone())
    .with_native_backend(NativeControlBackend {
        endpoint: target.endpoint.clone(),
        gate,
        codex_home: root.clone(),
    })?;
    let message: SavedMessage = serde_json::from_value(
        json!({"target":target,"content":{"kind":"agent","sender":target,"text":"A durable finding"},"delivery":"queue","generationGuard":null}),
    )?;
    let wake = store
        .lock()
        .await
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message,
            timing: TimingRule::After { seconds: 1 },
            expiry: ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(
        identity
            .wake_timing_worker()
            .ok_or("worker missing")?
            .run(shutdown.clone()),
    );
    let backend_database = database.clone();
    let (queue_sender, queue_receiver) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let init: Value =
            serde_json::from_str(socket.next().await.ok_or("missing init")??.to_text()?)?;
        socket
            .send(Message::Text(
                json!({"id":init.pointer("/id"),"result":{}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let initialized: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing initialized")??
                .to_text()?,
        )?;
        if initialized.pointer("/method").and_then(Value::as_str) != Some("initialized") {
            return Err("wrong handshake".into());
        }
        let read: Value =
            serde_json::from_str(socket.next().await.ok_or("missing read")??.to_text()?)?;
        if read.pointer("/method").and_then(Value::as_str) != Some("thread/read") {
            return Err("queue delivery skipped native residency check".into());
        }
        socket.send(Message::Text(json!({"id":read.pointer("/id"),"result":{"thread":{"id":"fixture-new-target","status":{"type":"idle"}}}}).to_string().into())).await?;
        let queued: Value =
            serde_json::from_str(socket.next().await.ok_or("missing queue")??.to_text()?)?;
        if queued.pointer("/method").and_then(Value::as_str) != Some("thread/queue/add") {
            return Err("explicit queue changed delivery semantics".into());
        }
        let mut evidence_connection = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&backend_database),
        )
        .await?;
        let evidence:String=sqlx::query_scalar("SELECT latest_attempt_json FROM mailbox_deliveries WHERE delivery_status='dispatching'").fetch_one(&mut evidence_connection).await?;
        let evidence: Value = serde_json::from_str(&evidence)?;
        if evidence
            .pointer("/effects/generation")
            .is_none_or(Value::is_null)
            || evidence.pointer("/effects/clientUserMessageId")
                != queued.pointer("/params/clientUserMessageId")
            || evidence
                .pointer("/effects/submission")
                .and_then(Value::as_str)
                != Some("dispatching")
        {
            return Err(
                "native generation and correlation were not durable before queue I/O".into(),
            );
        }
        evidence_connection.close().await?;
        if matches!(outcome, NativeOutcome::Accepted) {
            socket.send(Message::Text(json!({"id":queued.pointer("/id"),"result":{"queuedSubmission":{"id":"native-queue-receipt"}}}).to_string().into())).await?;
        }
        let _ = queue_sender.send(queued.clone());
        drop(socket);
        if matches!(
            outcome,
            NativeOutcome::ReconcileFound
                | NativeOutcome::ReconcileAbsent
                | NativeOutcome::ReconcileMismatch
        ) {
            let (stream, _) = listener.accept().await?;
            let mut socket = tokio_tungstenite::accept_async(stream).await?;
            let init: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .ok_or("missing reconcile init")??
                    .to_text()?,
            )?;
            socket
                .send(Message::Text(
                    json!({"id":init.get("id"),"result":{}}).to_string().into(),
                ))
                .await?;
            let _initialized = socket.next().await.ok_or("missing initialized")??;
            let request: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .ok_or("missing queue inspection")??
                    .to_text()?,
            )?;
            if request.get("method").and_then(Value::as_str) != Some("thread/queue/list")
                || request.pointer("/params/threadId") != queued.pointer("/params/threadId")
            {
                return Err(
                    "reconciliation mutated native state or inspected another target".into(),
                );
            }
            let mut item = json!({"id":"recovered-queue-id","input":queued.pointer("/params/input"),"clientUserMessageId":queued.pointer("/params/clientUserMessageId")});
            if matches!(outcome, NativeOutcome::ReconcileMismatch) {
                *item
                    .pointer_mut("/input/0/text")
                    .ok_or("fixture input text missing")? = json!("different message");
            }
            let data = if matches!(outcome, NativeOutcome::ReconcileAbsent) {
                json!([])
            } else {
                json!([item])
            };
            socket
                .send(Message::Text(
                    json!({"id":request.get("id"),"result":{"data":data,"nextCursor":null}})
                        .to_string()
                        .into(),
                ))
                .await?;
            if let Some(Ok(message)) =
                tokio::time::timeout(Duration::from_secs(2), socket.next()).await?
                && !message.is_close()
            {
                return Err("reconciliation sent another native request after inspection".into());
            }
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&database),
    )
    .await?;
    let (status,receipt)=tokio::time::timeout(Duration::from_secs(4),async {
        let mut poll=tokio::time::interval(Duration::from_millis(20));
        loop {
            poll.tick().await;
            let record:Option<(String,Option<String>)>=sqlx::query_as("SELECT delivery_status,accepted_receipt_json FROM mailbox_deliveries WHERE wakeup_id=? AND delivery_status IN ('accepted','uncertain')").bind(wake.definition.wakeup_id.as_str()).fetch_optional(&mut connection).await?;
            if let Some(record)=record{return Ok::<_,sqlx::Error>(record);}
        }
    }).await??;
    let request = tokio::time::timeout(Duration::from_secs(2), queue_receiver).await??;
    match outcome {
        NativeOutcome::Accepted => {
            let receipt: Value = serde_json::from_str(&receipt.ok_or("missing accepted receipt")?)?;
            if status != "accepted"
                || receipt
                    .pointer("/acceptance/submissionId")
                    .and_then(Value::as_str)
                    != Some("native-queue-receipt")
                || receipt.pointer("/clientUserMessageId")
                    != request.pointer("/params/clientUserMessageId")
            {
                return Err("actual receipt or correlation lost".into());
            }
        }
        NativeOutcome::ResponseLost
        | NativeOutcome::ReconcileFound
        | NativeOutcome::ReconcileAbsent
        | NativeOutcome::ReconcileMismatch => {
            if status != "uncertain" || receipt.is_some() {
                return Err("lost response fabricated acceptance".into());
            }
            if !store
                .lock()
                .await
                .eligible_delivery_ids(chrono::Utc::now().timestamp_millis() + 100000, 100)
                .await?
                .is_empty()
            {
                return Err("lost native response was eligible for replay".into());
            }
        }
    }
    if matches!(
        outcome,
        NativeOutcome::ReconcileFound
            | NativeOutcome::ReconcileAbsent
            | NativeOutcome::ReconcileMismatch
    ) {
        let (socket, server) = tokio::net::UnixStream::pair()?;
        let service = tokio::spawn(communication_service::serve_control_connection(
            server, identity,
        ));
        let mut client =
            communication_client::ControlClient::initialize(socket, "reconcile-fixture", "1")
                .await?;
        let delivery_id = request
            .pointer("/params/clientUserMessageId")
            .and_then(Value::as_str)
            .ok_or("correlation missing")?
            .to_owned()
            .try_into()?;
        let result = client
            .reconcile_delivery(communication_protocol::DeliveryShowRequest { delivery_id })
            .await?;
        if matches!(outcome, NativeOutcome::ReconcileFound) {
            let communication_protocol::DeliveryEvidence::Accepted { receipt, .. } =
                result.evidence
            else {
                return Err("matching queue evidence did not reconcile acceptance".into());
            };
            let communication_protocol::NativeSendAcceptance::QueueAccepted { submission_id } =
                receipt.acceptance
            else {
                return Err("reconciliation changed queue semantics".into());
            };
            if String::from(submission_id) != "recovered-queue-id" {
                return Err("reconciliation fabricated a queue identity".into());
            }
        } else if !matches!(
            result.evidence,
            communication_protocol::DeliveryEvidence::OutcomeUnknown { .. }
        ) {
            return Err("absence or content mismatch was treated as a definite outcome".into());
        }
        client.close().await?;
        service.await??;
    }
    tokio::time::timeout(Duration::from_secs(2), backend).await???;
    shutdown.cancel();
    worker.await?;
    connection.close().await?;
    drop(store);
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
