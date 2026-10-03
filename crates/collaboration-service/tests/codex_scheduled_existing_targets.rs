//! Existing scheduled Codex targets preserve and verify the declared workspace.
use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    CodexGeneration, MachineId, MachineLabel, PushHeaderFacts, PushId, PushKind, PushLineInput,
    PushOrigin, RouterLink, SessionRef, render_push_line,
};
use collaboration_service::{
    DeliveryFuture, DeliveryPrecondition, LoadPolicy, NativeControlBackend, NativeGenerationGate,
    ScheduledRunPayload, layer_zero::PreparedPush,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn scheduled_run_to_materialized_thread_preserves_declared_workspace()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use collaboration_service::{
        RunEvidenceDisposition, RunEvidenceSink, RunSubmission, ScheduledRunExecution,
    };
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "materialized-schedule-{}",
        agent_automation::RunId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let socket_path = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let service_id = "00000000-0000-4000-8000-000000000001";
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"materialized-thread"
    }))?;
    let generation: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch":service_id,"generation":1
    }))?;
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
    let gate = NativeGenerationGate::default();
    gate.activate(generation.clone(), socket_path.clone(), Some(schemas))?;
    let route = collaboration_service::CodexAppServerScheduledRuns::new(
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: root.clone(),
        },
        Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let run_id = agent_automation::RunId::generate();
    let schedule_id = agent_automation::ScheduleId::generate();
    let push_id = PushId::try_from(uuid::Uuid::now_v7().to_string())?;
    let prepared_line = render_push_line(&PushLineInput {
        link: RouterLink::new(MachineId::try_from(service_id.to_owned())?, push_id.clone()),
        machine_label: MachineLabel::try_from("fixture-host".to_owned())?,
        origin: PushOrigin::Router(PushKind::ScheduleRun),
        header_facts: PushHeaderFacts::ScheduleRun {
            schedule_id,
            run_id: run_id.clone(),
        },
        body: Some("materialized scheduled input".to_owned()),
    })?;
    let expected_push_id = push_id.clone();
    let expected_line = prepared_line.clone();
    if push_id.as_str() == run_id.as_str() {
        return Err("schedule push id must be distinct from RunId".into());
    }
    let native = tokio::spawn(async move {
        for methods in [
            &["thread/read"][..],
            &["thread/read"][..],
            &["thread/read"][..],
            &["thread/read", "turn/start"][..],
        ] {
            let (stream, _) = listener.accept().await?;
            let mut wire = tokio_tungstenite::accept_async(stream).await?;
            for expected in ["initialize", "initialized"]
                .into_iter()
                .chain(methods.iter().copied())
            {
                let frame = tokio::time::timeout(Duration::from_secs(3), wire.next())
                    .await?
                    .ok_or("native socket closed")??;
                let request: Value = serde_json::from_str(frame.to_text()?)?;
                if request["method"] != expected {
                    return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                        format!("expected {expected}, got {}", request["method"]).into(),
                    );
                }
                if expected == "initialized" {
                    continue;
                }
                if expected == "turn/start"
                    && request["params"]["clientUserMessageId"] != expected_push_id.as_str()
                {
                    return Err("scheduled turn lost push correlation id".into());
                }
                if expected == "turn/start"
                    && request["params"]["input"][0]["text"] != expected_line
                {
                    return Err("scheduled turn did not use the prepared push line".into());
                }
                let result = if expected == "thread/read" {
                    json!({"thread":{"id":"materialized-thread","cwd":"/work","status":{"type":"idle"}}})
                } else if expected == "turn/start" {
                    json!({"turn":{"id":"scheduled-turn"}})
                } else {
                    json!({})
                };
                wire.send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await?;
            }
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    struct EvidenceSink;
    impl RunEvidenceSink for EvidenceSink {
        fn record(
            &self,
            _: RouteEffectEvidence<SessionRef, CodexGeneration>,
        ) -> DeliveryFuture<'_, RunEvidenceDisposition> {
            Box::pin(async {
                Ok(RunEvidenceDisposition::Recorded {
                    timing: agent_automation::ExecutionTiming::start(1_000, 120),
                })
            })
        }
        fn record_stop_intent(&self) -> DeliveryFuture<'_, RunEvidenceDisposition> {
            Box::pin(async { Ok(RunEvidenceDisposition::AdmissionRefused) })
        }
    }
    let sink = EvidenceSink;
    let no_declared_cwd = route.prepare_existing_target(&target, "", &sink).await?;
    if no_declared_cwd.target != target {
        return Err("existing target with no declared cwd selected another thread".into());
    }
    if route
        .prepare_existing_target(&target, "/wrong-workspace", &sink)
        .await
        .is_ok()
    {
        return Err("scheduled preparation accepted a mismatched declared workspace".into());
    }
    let prepared = route
        .prepare_existing_target(&target, "/work", &sink)
        .await?;
    let inputs = serde_json::from_value(json!({
        "scheduleChangeId":agent_automation::ChangeId::generate(),
        "instructionRevisionId":agent_automation::RevisionId::generate(),
        "instructionText":"Check work","continuity":{"kind":"none"},
        "executionConfiguration":{"destination":{"kind":"ownedThread","target":target,"cwd":"/work"},
            "executionTimeoutSeconds":120,"model":"gpt-5.6-sol","effort":"medium"}
    }))?;
    let result = route
        .submit_run(
            collaboration_service::ScheduledRunSubmission {
                run_id,
                target,
                payload: ScheduledRunPayload::Existing {
                    prepared: PreparedPush {
                        push_id,
                        line: prepared_line.try_into()?,
                        load_policy: LoadPolicy::MayLoad,
                    },
                },
                precondition: DeliveryPrecondition::Unpinned,
                inputs,
                recorded: prepared.evidence,
            },
            &sink,
        )
        .await?;
    if !matches!(result, RunSubmission::Started(_)) {
        return Err(format!("materialized scheduled input was not started: {result:?}").into());
    }
    native.await??;
    std::fs::remove_file(socket_path)?;
    std::fs::remove_dir(root)?;
    Ok(())
}
