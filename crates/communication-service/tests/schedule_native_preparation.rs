//! Native preparation through real SDK/SQLite/WebSocket fixture; no Codex binary or model.
use automation_storage::AutomationStore;
use communication_client::ControlClient;
use communication_protocol::{
    CodexGeneration, EndpointDescription, InstructionCreateParams, InstructionText, OperationId,
    ScheduleCreateRequest, SchedulePrepareRequest, SessionRef,
};
use communication_service::{
    NativeControlBackend, NativeGenerationGate, ServiceIdentity, serve_control_connection,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::Message;
#[tokio::test]
async fn preparation_replay_uses_recorded_native_thread_without_forking_again()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_preparation(false).await
}
#[tokio::test]
async fn lost_preparation_response_is_retained_without_reallocation()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_preparation(true).await
}
async fn exercise_preparation(
    lose_response: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "schedule-native-fixture-{}",
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
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let service = tokio::spawn(serve_control_connection(server, identity.clone()));
    let mut client = ControlClient::initialize(socket, "prepare-fixture", "1").await?;
    let instruction = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("Check status".to_owned())?,
        })
        .await?;
    let schedule=client.create_schedule(serde_json::from_value::<ScheduleCreateRequest>(json!({"operationId":OperationId::generate(),"definition":{"instructionId":instruction.instruction_id,"timing":{"kind":"interval","seconds":60},"enabled":false,"destination":{"kind":"unprepared"},"executionTimeoutSeconds":null}}))?).await?;
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let init: Value =
            serde_json::from_str(socket.next().await.ok_or("missing init")??.to_text()?)?;
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
                .ok_or("missing thread start")??
                .to_text()?,
        )?;
        if request.get("method").and_then(Value::as_str) != Some("thread/start")
            || request.pointer("/params/cwd").and_then(Value::as_str) != Some("/fresh-fixture")
        {
            return Err("wrong preparation call or cwd".into());
        }
        if !lose_response {
            socket.send(Message::Text(json!({"id":request.get("id"),"result":{"thread":{"id":"prepared-new-thread","cwd":"/fresh-fixture"},"cwd":"/fresh-fixture","model":"gpt-5.6-luna"}}).to_string().into())).await?;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let request: SchedulePrepareRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"scheduleId":schedule.schedule_id,"destination":{"kind":"fresh","endpoint":target.endpoint,"cwd":"/fresh-fixture"}}),
    )?;
    let first = tokio::time::timeout(
        Duration::from_secs(3),
        client.prepare_schedule(request.clone()),
    )
    .await?;
    if lose_response {
        let error = match first {
            Err(communication_client::ScheduleClientError::Rejected(error)) => error,
            _ => return Err("lost allocation response lacked typed evidence".into()),
        };
        if !matches!(&error.effects,communication_protocol::ScheduleEffects::Native{evidence} if matches!(evidence.allocation,communication_protocol::PreparationEffect::Unknown))
        {
            return Err("lost allocation response invented no effect".into());
        }
        let (socket, server) = tokio::net::UnixStream::pair()?;
        let replay_service = tokio::spawn(serve_control_connection(server, identity));
        let mut replay_client = ControlClient::initialize(socket, "prepare-replay", "1").await?;
        let replay = replay_client.prepare_schedule(request).await;
        let replay = match replay {
            Err(communication_client::ScheduleClientError::Rejected(error)) => error,
            _ => return Err("uncertain preparation replayed allocation".into()),
        };
        if serde_json::to_value(error)? != serde_json::to_value(replay)? {
            return Err("retained preparation failure changed on replay".into());
        }
        replay_client.close().await?;
        replay_service.await??;
    } else {
        let first = first?;
        let replay = client.prepare_schedule(request).await?;
        if serde_json::to_value(&first)? != serde_json::to_value(replay)?
            || first.definition.enabled
        {
            return Err("preparation replay or activation state changed".into());
        }
        match first.definition.destination {
            communication_protocol::ExecutionDestination::OwnedThread { target, .. }
                if String::from(target.session_id.clone()) == "prepared-new-thread" => {}
            _ => return Err("native thread identity not bound".into()),
        }
    }
    client.close().await?;
    service.await??;
    backend.await??;
    drop(store);
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
