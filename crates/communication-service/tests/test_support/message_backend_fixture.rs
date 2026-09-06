//! Real socket fixture with scripted native responses; never a live model backend.
use communication_client::{ClientError, ControlClient};
use communication_protocol::{
    CodexGeneration, EndpointDescription, MessageContent, MessageDelivery, NativeSendParams,
    NativeSendReceipt, SessionRef,
};
use communication_service::{
    NativeControlBackend, NativeGenerationGate, ServiceIdentity, new_service_uuid,
    serve_control_connection,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::Message;

pub enum NativeReply {
    Result(Value),
    Reject,
    Disconnect,
}
pub struct NativeStep {
    pub method: &'static str,
    pub reply: NativeReply,
}
pub struct MessageScenario {
    pub delivery: MessageDelivery,
    pub steps: Vec<NativeStep>,
}
type FixtureError = Box<dyn std::error::Error + Send + Sync>;
pub async fn exercise(
    scenario: MessageScenario,
) -> Result<(Result<NativeSendReceipt, ClientError>, Vec<Value>), FixtureError> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "message-fixture-{}",
        String::from(new_service_uuid()?)
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let socket_path = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let service_id = "00000000-0000-4000-8000-000000000001";
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let target: SessionRef = serde_json::from_value(
        json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"target"}),
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
        json!({"endpoint":target.endpoint,"label":"Fixture","availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]}),
    )?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation.clone(), socket_path.clone(), Some(schemas))?;
    let identity = ServiceIdentity::new(
        service_id,
        service_id,
        &format!("sha256:{}", "a".repeat(64)),
    )?
    .with_endpoints(vec![description])?
    .with_native_backend(NativeControlBackend {
        codex_home: root.clone(),
        endpoint: target.endpoint.clone(),
        gate,
    })?;
    let (client, server) = tokio::net::UnixStream::pair()?;
    let service = tokio::spawn(serve_control_connection(server, identity));
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let init: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing native frame")??
                .to_text()?,
        )?;
        assert_eq!(
            init.get("method").and_then(Value::as_str),
            Some("initialize")
        );
        socket
            .send(Message::Text(
                json!({"id":init.get("id").ok_or("missing initialization ID")?,"result":{}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let initialized: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing native frame")??
                .to_text()?,
        )?;
        assert_eq!(
            initialized.get("method").and_then(Value::as_str),
            Some("initialized")
        );
        let mut requests = Vec::new();
        for step in scenario.steps {
            let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await?
                .ok_or("native socket closed before expected operation")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            assert_eq!(
                request.get("method").and_then(Value::as_str),
                Some(step.method)
            );
            let response = match step.reply {
                NativeReply::Result(result) => {
                    json!({"id":request.get("id").ok_or("missing request ID")?,"result":result})
                }
                NativeReply::Reject => {
                    json!({"id":request.get("id").ok_or("missing request ID")?,"error":{"code":-32602,"message":"Fixture rejection"}})
                }
                NativeReply::Disconnect => {
                    requests.push(request);
                    return Ok::<_, FixtureError>(requests);
                }
            };
            requests.push(request);
            socket
                .send(Message::Text(response.to_string().into()))
                .await?;
        }
        // No fallback, extra mutation or retry may follow the scripted terminal response.
        if let Some(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), socket.next()).await?
        {
            assert!(frame.is_close(), "unexpected extra native frame: {frame}");
        }
        Ok::<_, FixtureError>(requests)
    });
    let mut client = ControlClient::initialize(client, "message-proof", "1").await?;
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        client.send_agent_message(NativeSendParams {
            target: target.clone(),
            generation,
            message: MessageContent::Agent {
                sender: target,
                text: "A finding".to_owned().try_into()?,
            },
            delivery: scenario.delivery,
            client_user_message_id: None,
        }),
    )
    .await?;
    client.close().await?;
    service.await??;
    let requests = backend.await??;
    std::fs::remove_file(socket_path)?;
    std::fs::remove_dir(root)?;
    Ok((result, requests))
}
pub fn read(status: &str) -> NativeStep {
    NativeStep {
        method: "thread/read",
        reply: NativeReply::Result(json!({"thread":{"id":"target","status":{"type":status}}})),
    }
}
pub fn error_data(result: Result<NativeSendReceipt, ClientError>) -> Result<Value, &'static str> {
    match result {
        Err(ClientError::Rejected {
            data: Some(data), ..
        }) => Ok(data),
        _ => Err("expected typed rejection"),
    }
}
