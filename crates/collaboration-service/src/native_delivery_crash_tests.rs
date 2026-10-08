//! Abrupt sender process loss preserves delivery identity and never manufactures acceptance.
use super::*;
use crate::{
    CodexAppServerDeliveryRoute, NativeControlBackend, SessionDeliveryRoute, SessionDeliveryRouter,
    SessionMessageDelivery,
};
use collaboration_client::ControlClient;
use collaboration_protocol::{
    DeliveryDisposition, DeliveryEvidence, DeliveryShowRequest, OperationId, PushId,
    RouterOriginRef, SavedMessage,
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
use tokio_util::sync::CancellationToken;

const ROOT_ENV: &str = "NATIVE_DELIVERY_TEST_CRASH_ROOT";
const STAGE_ENV: &str = "NATIVE_DELIVERY_TEST_CRASH_STAGE";
const ID_ENV: &str = "NATIVE_DELIVERY_TEST_CRASH_ID";
const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
const OLD_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const NEW_EPOCH: &str = "00000000-0000-4000-8000-000000000003";
const CRASH_EXIT: i32 = 93;
type TestResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

pub(super) fn checkpoint(stage: &str) {
    if std::env::var(STAGE_ENV).as_deref() == Ok(stage) {
        std::process::exit(CRASH_EXIT);
    }
}

fn native_delivery_crash_root(fixture_id: &OperationId) -> PathBuf {
    let uuid_tail: String = fixture_id
        .as_str()
        .chars()
        .filter(|character| *character != '-')
        .skip(16)
        .collect();
    std::env::temp_dir().join(format!("wake-crash-{uuid_tail}"))
}

#[tokio::test]
async fn native_delivery_process_loss_preserves_uncertainty_without_resend() -> TestResult<()> {
    for stage in ["intent-persisted", "receipt-before-commit"] {
        let fixture_id = OperationId::generate();
        let root = native_delivery_crash_root(&fixture_id);
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "wakeup_delivery_sender::crash_tests::native_delivery_crash_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env(ROOT_ENV, &root)
                .env(STAGE_ENV, stage)
                .env(ID_ENV, fixture_id.as_str())
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        if output.status.code() != Some(CRASH_EXIT) {
            return Err(format!(
                "{stage}: checkpoint not reached: {:?}; {}; {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let delivery: DeliveryId =
            serde_json::from_slice(&std::fs::read(root.join("delivery-id.json"))?)?;
        let push_id: PushId = serde_json::from_slice(&std::fs::read(root.join("push-id.json"))?)?;
        let witness_path = root.join("native-receipt.json");
        if witness_path.exists() != (stage == "receipt-before-commit") {
            return Err(
                format!("{stage}: native receipt witness disagrees with checkpoint").into(),
            );
        }
        if witness_path.exists() {
            let witness: Value = serde_json::from_slice(&std::fs::read(&witness_path)?)?;
            if witness.get("clientUserMessageId") != Some(&json!(push_id)) {
                return Err("native mutation lost stable delivery correlation".into());
            }
        }
        let store = Arc::new(Mutex::new(
            AutomationStore::open(&root.join("automation.sqlite")).await?,
        ));
        let before = store
            .lock()
            .await
            .read_delivery::<SessionRef, CodexGeneration, crate::stored_delivery_receipt::StoredDeliveryReceipt>(&delivery)
            .await?;
        let before_attempt = before.attempt.ok_or("persisted attempt missing")?;
        if before.status != agent_automation::DeliveryStatus::Dispatching
            || before.receipt.is_some()
        {
            return Err("abrupt exit did not leave the uncommitted native outcome".into());
        }
        let identity = identity(&root, Arc::clone(&store), NEW_EPOCH)?;
        let stop = CancellationToken::new();
        let worker = tokio::spawn(
            identity
                .wake_timing_worker()
                .ok_or("wake worker missing")?
                .run(stop.clone()),
        );
        let (socket, peer) = tokio::net::UnixStream::pair()?;
        let service = tokio::spawn(crate::serve_control_connection(peer, identity));
        let mut client = ControlClient::initialize(socket, "delivery-recovery", "1").await?;
        let observed = tokio::time::timeout(Duration::from_secs(3), async {
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            loop {
                interval.tick().await;
                let current = client
                    .read_delivery(DeliveryShowRequest {
                        delivery_id: delivery.clone(),
                    })
                    .await?;
                if matches!(current.disposition, DeliveryDisposition::Uncertain) {
                    return Ok::<_, collaboration_client::WakeClientError>(current);
                }
            }
        })
        .await??;
        let same_attempt = matches!(&observed.evidence, DeliveryEvidence::OutcomeUnknown { attempt_id, .. } if *attempt_id == before_attempt.attempt_id);
        let eligible = store
            .lock()
            .await
            .eligible_delivery_ids(chrono::Utc::now().timestamp_millis() + 60_000, 16)
            .await?;
        stop.cancel();
        worker.await?;
        client.close().await?;
        service.await??;
        if observed.delivery_id != delivery || !same_attempt || !eligible.is_empty() {
            return Err(
                "recovery changed attempt identity or made uncertain delivery retryable".into(),
            );
        }
        let recovered = store
            .lock()
            .await
            .read_delivery::<SessionRef, CodexGeneration, crate::stored_delivery_receipt::StoredDeliveryReceipt>(&delivery)
            .await?;
        if recovered.receipt.is_some() {
            return Err("recovery fabricated a lost native acceptance receipt".into());
        }
        drop(store);
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                return Err("unexpected fixture directory".into());
            }
            std::fs::remove_file(entry.path())?;
        }
        std::fs::remove_dir(root)?;
    }
    Ok(())
}

#[tokio::test]
async fn interrupted_wake_attempt_survives_shared_restore_and_worker_delivers_once()
-> TestResult<()> {
    let fixture_id = OperationId::generate();
    let root = native_delivery_crash_root(&fixture_id);
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "wakeup_delivery_sender::crash_tests::native_delivery_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env(ROOT_ENV, &root)
            .env(STAGE_ENV, "fire-committed")
            .env(ID_ENV, fixture_id.as_str())
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if output.status.code() != Some(CRASH_EXIT) {
        return Err(format!(
            "fire-committed: checkpoint not reached: {:?}; {}; {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let delivery_id: DeliveryId =
        serde_json::from_slice(&std::fs::read(root.join("delivery-id.json"))?)?;
    let expected_push_id: PushId =
        serde_json::from_slice(&std::fs::read(root.join("push-id.json"))?)?;
    let witness_path = root.join("native-receipt.json");
    if witness_path.exists() {
        return Err("fire-committed child dispatched before exiting".into());
    }

    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let globally_settled = store
        .lock()
        .await
        .settle_interrupted_pushes(chrono::Utc::now())
        .await?;
    if globally_settled != 0 {
        return Err("shared startup recovery settled the Wake-owned attempt".into());
    }
    let before = store
        .lock()
        .await
        .read_delivery::<SessionRef, CodexGeneration, crate::stored_delivery_receipt::StoredDeliveryReceipt>(
            &delivery_id,
        )
        .await?;
    if before.status != agent_automation::DeliveryStatus::Pending || before.attempt.is_some() {
        return Err("fire commit did not leave one undispatched mailbox row".into());
    }
    let push_before = store
        .lock()
        .await
        .get_push_record(&expected_push_id)
        .await?
        .ok_or("committed wake push missing after restart")?;
    if push_before.delivery_state != automation_storage::PushDeliveryState::Attempted {
        return Err("Wake-owned attempt did not remain available to its worker".into());
    }

    let listener = tokio::net::UnixListener::bind(root.join("native.sock"))?;
    let (correlation_sender, correlation_receiver) = tokio::sync::oneshot::channel();
    let native = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let init: Value =
            serde_json::from_str(socket.next().await.ok_or("missing init")??.to_text()?)?;
        socket
            .send(Message::Text(
                json!({"id":init["id"],"result":{}}).to_string().into(),
            ))
            .await?;
        let _initialized = socket.next().await.ok_or("missing initialized")??;
        let read: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing native read")??
                .to_text()?,
        )?;
        if read.get("method") != Some(&json!("thread/read")) {
            return Err("wake delivery skipped the native residency read".into());
        }
        socket
            .send(Message::Text(
                json!({"id":read["id"],"result":{"thread":{"id":"fixture-thread","status":{"type":"idle"}}}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let start: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing native start")??
                .to_text()?,
        )?;
        if start.get("method") != Some(&json!("turn/start")) {
            return Err("wake delivery did not use the saved auto mode".into());
        }
        let correlation = start
            .pointer("/params/clientUserMessageId")
            .and_then(Value::as_str)
            .ok_or("wake push correlation missing")?
            .to_owned();
        socket
            .send(Message::Text(
                json!({"id":start["id"],"result":{"turn":{"id":"fixture-turn"}}})
                    .to_string()
                    .into(),
            ))
            .await?;
        correlation_sender
            .send(correlation)
            .map_err(|_| "wake correlation receiver dropped")?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let identity = identity(&root, Arc::clone(&store), NEW_EPOCH)?;
    let sender = WakeDeliverySender {
        delivery: identity
            .session_delivery
            .ok_or("session delivery missing")?,
        configuration: identity.configuration,
        machine_identity: identity.machine_identity,
    };
    tokio::time::timeout(
        Duration::from_secs(5),
        sender.dispatch(Arc::clone(&store), delivery_id.clone()),
    )
    .await??;
    let observed_correlation =
        tokio::time::timeout(Duration::from_secs(5), correlation_receiver).await??;
    if observed_correlation != expected_push_id.as_str() {
        native.await??;
        return Err("restart dispatched a different push id".into());
    }
    native.await??;
    let recovered = store
        .lock()
        .await
        .read_delivery::<
            SessionRef,
            CodexGeneration,
            crate::stored_delivery_receipt::StoredDeliveryReceipt,
        >(&delivery_id)
        .await?;
    if recovered.status != agent_automation::DeliveryStatus::Accepted || recovered.receipt.is_none()
    {
        return Err("restart delivery did not commit its accepted mailbox result".into());
    }
    let push_after = store
        .lock()
        .await
        .get_push_record(&expected_push_id)
        .await?
        .ok_or("wake push missing after restart delivery")?;
    if push_after.push_id != push_before.push_id
        || push_after.delivery_state != automation_storage::PushDeliveryState::Delivered
    {
        return Err("restart did not settle the original wake push".into());
    }
    drop(store);
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            return Err("unexpected fixture directory".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}

#[tokio::test]
#[ignore = "owned sender subprocess exits at the selected durable/native boundary"]
async fn native_delivery_crash_child() -> TestResult<()> {
    let fixture_id: OperationId = std::env::var(ID_ENV)?.try_into()?;
    let root = PathBuf::from(std::env::var(ROOT_ENV)?);
    let expected_root = native_delivery_crash_root(&fixture_id);
    if root != expected_root {
        return Err("refusing non-fixture sender root".into());
    }
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let saved: SavedMessage = serde_json::from_value(json!({
        "target":{"endpoint":{"serviceId":SERVICE,"endpointId":"codex-local"},"sessionId":"fixture-thread"},
        "content":{"kind":"humanUser","text":"fixture work"},"delivery":"auto","generationGuard":null
    }))?;
    let wake = store
        .lock()
        .await
        .create_wakeup(&automation_storage::WakeCreate {
            operation_id: OperationId::generate(),
            message: saved,
            timing: agent_automation::TimingRule::After { seconds: 1 },
            expiry: agent_automation::ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    let automation_storage::WakeEvaluation::Fired {
        delivery_id, fire, ..
    } = store
        .lock()
        .await
        .evaluate_wakeup::<SavedMessage>(&wake.definition.wakeup_id, 1000, build_wake_push_draft)
        .await?
    else {
        return Err("fixture firing missing".into());
    };
    std::fs::write(
        root.join("delivery-id.json"),
        serde_json::to_vec(&delivery_id)?,
    )?;
    let origin = RouterOriginRef::Wake {
        wakeup_id: fire.wakeup_id,
        occurrence_id: fire.occurrence_id,
    };
    let push = store
        .lock()
        .await
        .get_push_record_by_origin_reference(&origin)
        .await?
        .ok_or("wake push missing in crash child")?;
    std::fs::write(
        root.join("push-id.json"),
        serde_json::to_vec(&push.push_id)?,
    )?;
    if std::env::var(STAGE_ENV).as_deref() == Ok("fire-committed") {
        store
            .lock()
            .await
            .mark_push_attempted(&push.push_id)
            .await?;
    }
    checkpoint("fire-committed");
    let listener = tokio::net::UnixListener::bind(root.join("native.sock"))?;
    let witness = root.join("native-receipt.json");
    let _native = tokio::spawn(async move {
        let (socket, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(socket).await?;
        let init: Value =
            serde_json::from_str(socket.next().await.ok_or("missing init")??.to_text()?)?;
        socket
            .send(Message::Text(
                json!({"id":init["id"],"result":{}}).to_string().into(),
            ))
            .await?;
        let _initialized = socket.next().await.ok_or("missing initialized")??;
        for method in ["thread/read", "turn/start"] {
            let request: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .ok_or("missing native request")??
                    .to_text()?,
            )?;
            if request.get("method") != Some(&json!(method))
                || request.pointer("/params/threadId") != Some(&json!("fixture-thread"))
            {
                return Err("wrong native operation or thread".into());
            }
            let result = if method == "thread/read" {
                json!({"thread":{"id":"fixture-thread","status":{"type":"idle"}}})
            } else {
                std::fs::write(
                    &witness,
                    serde_json::to_vec(
                        &json!({"clientUserMessageId":request.pointer("/params/clientUserMessageId")}),
                    )?,
                )?;
                json!({"turn":{"id":"fixture-turn"}})
            };
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        std::future::pending::<()>().await;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let identity = identity(&root, Arc::clone(&store), OLD_EPOCH)?;
    let sender = WakeDeliverySender {
        delivery: identity
            .session_delivery
            .ok_or("session delivery missing")?,
        configuration: identity.configuration,
        machine_identity: identity.machine_identity,
    };
    sender.dispatch(store, delivery_id).await?;
    Err("sender did not stop at selected checkpoint".into())
}

fn identity(
    root: &Path,
    store: Arc<Mutex<AutomationStore>>,
    epoch: &str,
) -> TestResult<crate::ServiceIdentity> {
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
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":epoch,"generation":1}))?;
    let endpoint = serde_json::from_value(json!({"serviceId":SERVICE,"endpointId":"codex-local"}))?;
    let description = serde_json::from_value(
        json!({"endpoint":endpoint,"label":"fixture","availability":{"state":"available","observedAt":"2026-09-09T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]}),
    )?;
    let gate = crate::NativeGenerationGate::default();
    gate.activate(generation, root.join("native.sock"), Some(schemas))?;
    let backend = NativeControlBackend {
        endpoint,
        gate,
        codex_home: root.to_owned(),
    };
    let identity = crate::ServiceIdentity::new(SERVICE, epoch)
        .map_err(std::io::Error::other)?
        .with_automation_store(store)
        .with_endpoints(vec![description])
        .map_err(std::io::Error::other)?
        .with_native_backend(backend.clone())
        .map_err(std::io::Error::other)?;
    let route: Arc<dyn SessionDeliveryRoute> = Arc::new(CodexAppServerDeliveryRoute::new(
        collaboration_protocol::UuidIdentity::try_from(SERVICE.to_owned())?,
        identity.endpoint_directory(),
        backend,
        Arc::new(crate::UnmaterializedThreadHolder::new()),
    ));
    let delivery: Arc<dyn SessionMessageDelivery> =
        Arc::new(SessionDeliveryRouter::new(vec![route]));
    Ok(identity.with_session_delivery(delivery))
}
