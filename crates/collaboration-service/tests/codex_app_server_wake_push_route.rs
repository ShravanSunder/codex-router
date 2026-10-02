use agent_automation::{ExpiryRule, RouteEffectEvidence, SubmissionEffect, TimingRule};
use automation_storage::{AutomationStore, WakeCreate, WakeEvaluation};
use collaboration_protocol::{
    AcceptedResumeEffect, CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, MachineId,
    MachineLabel, MessageDelivery, MessageInputKind, MessageRepresentation, PushDeliveryState,
    PushKind, PushLineInput, RouterLink, RouterOriginRef, SessionRef, UuidIdentity,
    render_push_line,
};
use collaboration_service::{
    AttemptEvidenceSink, CodexAppServerDeliveryRoute, DeliveryClientReceipt, DeliveryContractError,
    DeliveryFuture, DeliveryPrecondition, EndpointDirectory, LoadPolicy, NativeControlBackend,
    NativeGenerationGate, SessionDeliveryRoute,
    layer_zero::{DeliveryRequest as PreparedDeliveryRequest, PreparedPush},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_tungstenite::tungstenite::Message;
#[path = "support/wake_push_draft.rs"]
mod wake_push_test_support;

struct RecordingEvidenceSink(Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>);

impl AttemptEvidenceSink for RecordingEvidenceSink {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        let result = self
            .0
            .lock()
            .map(|mut records| records.push(evidence))
            .map_err(|_| DeliveryContractError::EvidencePersistence);
        Box::pin(async move { result })
    }
}

#[tokio::test]
async fn stored_wake_push_resumes_unloaded_codex_thread_with_its_push_identity()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = tempfile::tempdir()?;
    let socket_path = root.path().join("native.sock");
    let automation_path = root.path().join("automation.sqlite");
    let mut automation = AutomationStore::open(&automation_path).await?;
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"unloaded-wake-thread"
    }))?;
    let saved_message: collaboration_protocol::SavedMessage = serde_json::from_value(json!({
        "target":target,
        "content":{"kind":"agent","sender":target,"text":"A durable wake"},
        "delivery":"auto",
        "generationGuard":null
    }))?;
    let wake = automation
        .create_wakeup(&WakeCreate {
            operation_id: collaboration_protocol::OperationId::generate(),
            message: saved_message,
            timing: TimingRule::After { seconds: 1 },
            expiry: ExpiryRule::None,
            now_ms: 1000,
        })
        .await?;
    let fire = match automation
        .evaluate_wakeup::<collaboration_protocol::SavedMessage>(
            &wake.definition.wakeup_id,
            5000,
            wake_push_test_support::build_test_wake_push_draft,
        )
        .await?
    {
        WakeEvaluation::Fired { fire, .. } => fire,
        _ => return Err("wake did not persist a firing and push record".into()),
    };
    let origin = RouterOriginRef::Wake {
        wakeup_id: fire.wakeup_id,
        occurrence_id: fire.occurrence_id,
    };
    let stored_push = automation
        .get_push_record_by_origin_reference(&origin)
        .await?
        .ok_or("fired wake did not persist its push record")?;
    if stored_push.kind != PushKind::Wake
        || stored_push.target != target
        || stored_push.delivery_state != PushDeliveryState::Pending
    {
        return Err("stored wake push changed its kind, target, or pending state".into());
    }
    let line = render_push_line(&PushLineInput {
        link: RouterLink::new(
            MachineId::from(service_id.clone()),
            stored_push.push_id.clone(),
        ),
        machine_label: MachineLabel::try_from("fixture-host".to_owned())?,
        origin: stored_push.origin.clone(),
        header_facts: stored_push.header_facts.clone(),
        body: stored_push.body.clone(),
    })?;
    let push_id = stored_push.push_id.clone();
    let expected_push_id = push_id.clone();
    let target_session_id = String::from(target.session_id.clone());
    let expected_line = line.clone();

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
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
    let endpoints = EndpointDirectory::new(service_id.clone());
    endpoints.publish(serde_json::from_value(json!({
        "endpoint":target.endpoint,
        "label":"Wake route fixture",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]
    }))?)?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation, socket_path.clone(), Some(schemas))?;
    let route = CodexAppServerDeliveryRoute::new(
        service_id.clone(),
        endpoints,
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: root.path().to_owned(),
        },
        Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let observed_evidence = Arc::new(RecordingEvidenceSink(Mutex::new(Vec::new())));
    let server = tokio::spawn(async move {
        let (stream, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept()).await??;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        for expected_method in [
            "initialize",
            "initialized",
            "thread/read",
            "thread/resume",
            "turn/start",
        ] {
            let frame = tokio::time::timeout(Duration::from_secs(3), socket.next())
                .await?
                .ok_or("native app-server connection closed early")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            if request.get("method").and_then(Value::as_str) != Some(expected_method) {
                return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                    format!(
                        "expected native method {expected_method}, received {:?}",
                        request.get("method")
                    )
                    .into(),
                );
            }
            if expected_method == "initialized" {
                continue;
            }
            if expected_method == "thread/read"
                && request.pointer("/params/threadId").and_then(Value::as_str)
                    != Some(target_session_id.as_str())
            {
                return Err("unloaded-thread inspection targeted another session".into());
            }
            if expected_method == "thread/resume"
                && (request.pointer("/params/threadId").and_then(Value::as_str)
                    != Some(target_session_id.as_str())
                    || request.pointer("/params/excludeTurns") != Some(&json!(true)))
            {
                return Err("resume did not target the unloaded thread".into());
            }
            if expected_method == "turn/start"
                && (request.pointer("/params/threadId").and_then(Value::as_str)
                    != Some(target_session_id.as_str())
                    || request
                        .pointer("/params/clientUserMessageId")
                        .and_then(Value::as_str)
                        != Some(expected_push_id.as_str())
                    || request
                        .pointer("/params/input/0/text")
                        .and_then(Value::as_str)
                        != Some(expected_line.as_str()))
            {
                return Err("wake input lost its stored push ID, target, or rendered line".into());
            }
            let result = match expected_method {
                "initialize" => json!({}),
                "thread/read" => {
                    json!({"thread":{"id":target_session_id,"cwd":"/work","status":{"type":"notLoaded"}}})
                }
                "thread/resume" => {
                    json!({"thread":{"id":target_session_id,"cwd":"/work","turns":[]}})
                }
                _ => json!({"turn":{"id":"wake-turn"}}),
            };
            socket
                .send(Message::Text(
                    json!({"id":request.get("id"),"result":result})
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        if tokio::time::timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_ok()
        {
            return Err("wake delivery opened a second native connection".into());
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    let receipt = route
        .deliver(
            PreparedDeliveryRequest {
                payload: PreparedPush {
                    push_id: push_id.clone(),
                    line: line.try_into()?,
                    load_policy: LoadPolicy::MayLoad,
                },
                target: target.clone(),
                mode: MessageDelivery::Auto,
                precondition: DeliveryPrecondition::Unpinned,
                correlation: DeliveryCorrelationId::try_from(push_id.as_str().to_owned())?,
                attempt: agent_automation::AttemptId::generate(),
            },
            observed_evidence.as_ref(),
        )
        .await?;
    if receipt.outcome != DeliveryOutcome::StartedOrSteered
        || receipt.reachability != Some(collaboration_protocol::SessionReachability::CodexAppServer)
        || !matches!(
            receipt.client.as_ref(),
            Some(DeliveryClientReceipt::CodexAppServer(native))
                if native.target == target
                    && String::from(native.client_user_message_id.clone()) == push_id.as_str()
                    && native.resume_effect == AcceptedResumeEffect::Accepted
                    && native.input_kind == MessageInputKind::Agent
                    && native.representation == MessageRepresentation::DeclaredAgentText
        )
    {
        return Err(format!("stored wake push was not resumed and started: {receipt:?}").into());
    }
    {
        let effects = observed_evidence
            .0
            .lock()
            .map_err(|_| "evidence lock unavailable")?;
        if effects.len() != 2
            || !matches!(&effects[0], RouteEffectEvidence::CodexAppServer(native)
            if native.client_user_message_id.as_deref() == Some(push_id.as_str())
                && native.resume == agent_automation::PreparationEffect::Unknown
                && native.submission == SubmissionEffect::Dispatching)
            || !matches!(&effects[1], RouteEffectEvidence::CodexAppServer(native)
            if native.client_user_message_id.as_deref() == Some(push_id.as_str())
                && native.resume == agent_automation::PreparationEffect::Accepted
                && native.submission == SubmissionEffect::Accepted)
        {
            return Err(
                "wake route evidence did not retain the stored push and accepted resume".into(),
            );
        }
    }
    server.await??;
    automation.close().await?;
    Ok(())
}
