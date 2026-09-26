//! A missing Codex thread has no replayable native input and no persisted holder identity.
use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, EndpointDescription, MessageContent,
    MessageDelivery, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, CodexAppServerDeliveryRoute, DeliveryFuture, DeliveryPrecondition,
    DeliveryRequest, EndpointDirectory, NativeControlBackend, NativeGenerationGate,
    SessionDeliveryRoute, UnmaterializedThreadHolder,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::Message;

struct DiscardEvidence;
impl AttemptEvidenceSink for DiscardEvidence {
    fn record(
        &self,
        _: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn missing_thread_without_holder_is_known_not_submitted()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let socket =
        std::path::PathBuf::from(format!("/tmp/codex-missing-{}.sock", std::process::id()));
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"00000000-0000-4000-8000-000000000099"
    }))?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "ThreadTurnsList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
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
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Fixture","availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]
    }))?;
    let directory = EndpointDirectory::new(service_id.clone());
    directory.publish(description)?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation, socket.clone(), Some(schemas))?;
    let route = CodexAppServerDeliveryRoute::new(
        service_id,
        directory,
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: "/tmp".into(),
        },
        Arc::new(UnmaterializedThreadHolder::new()),
    );
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut wire = tokio_tungstenite::accept_async(stream).await?;
        for expected in ["initialize", "initialized", "thread/read"] {
            let frame = tokio::time::timeout(Duration::from_secs(3), wire.next())
                .await?
                .ok_or("connection closed")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            if request["method"] != expected {
                return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                    format!("unexpected method: {}", request["method"]).into(),
                );
            }
            if expected == "initialized" {
                continue;
            }
            let response = if expected == "initialize" {
                json!({"id":request["id"],"result":{}})
            } else {
                json!({"id":request["id"],"error":{"code":-32600,"message":"thread not loaded: 00000000-0000-4000-8000-000000000099"}})
            };
            wire.send(Message::Text(response.to_string().into()))
                .await?;
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let receipt = route
        .deliver(
            DeliveryRequest {
                target,
                message: MessageContent::Router {
                    text: "hello".to_owned().try_into()?,
                },
                mode: MessageDelivery::Auto,
                precondition: DeliveryPrecondition::Unpinned,
                correlation: DeliveryCorrelationId::generate(),
                attempt: agent_automation::AttemptId::generate(),
            },
            &DiscardEvidence,
        )
        .await?;
    if !matches!(receipt.outcome, DeliveryOutcome::NotSubmitted { retryable: false, ref reason }
        if reason == "thread not found or never started; if it was created without a first message, it was lost when the Host restarted — create it again")
    {
        return Err(format!("missing thread outcome: {:?}", receipt.outcome).into());
    }
    backend.await??;
    std::fs::remove_file(socket)?;
    Ok(())
}
