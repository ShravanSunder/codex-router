//! Established Control transport with a controlled admission gate and a real debug Codex backend.
//! Host discovery and native relay publication have separate live acceptance coverage.
use super::{
    proof_context::{ProofContext, ProofResult},
    summary_failure_recovery::PortableRunProof,
};
use automation_storage::AutomationStore;
use communication_client::{ControlClient, WakeWaitError};
use communication_protocol::{
    ChannelDescription, CodexGeneration, DeliveryEvidence, DeliveryListRequest,
    DeliveryShowRequest, EndpointAvailability, EndpointDescription, EndpointRef,
    ExecutionDestination, ExpiryRequest, ImportedContinuity, MessageContent, MessageDelivery,
    NativeCarrier, OperationId, RunListRequest, SavedMessage, ScheduleImportRequest, SessionRef,
    TimingRequest, WakeMutationRequest, WakeSendRequest, WakeShowRequest,
};
use communication_service::{NativeControlBackend, NativeGenerationGate, ServiceIdentity};
use serde_json::{Value, json};
use std::{os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub async fn exercise(proof: &mut ProofContext, portable: PortableRunProof) -> ProofResult<()> {
    let native_target = proof
        .start_thread("Luna durable delivery recipient")
        .await?;
    let directory = proof.root.join("recovery-service");
    std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&directory.join("automation.sqlite")).await?,
    ));
    let service_id = uuid::Uuid::now_v7().to_string();
    let epoch = uuid::Uuid::now_v7().to_string();
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":epoch,"generation":1}))?;
    let endpoint = EndpointRef {
        service_id: service_id.clone().try_into()?,
        endpoint_id: proof.endpoint.endpoint_id.clone(),
    };
    let target = SessionRef {
        endpoint: endpoint.clone(),
        session_id: native_target.session_id,
    };
    let mut description = EndpointDescription {
        endpoint: endpoint.clone(),
        label: "Recovery acceptance".to_owned().try_into()?,
        availability: EndpointAvailability::Unavailable {
            observed_at: timestamp()?,
            reason: "Backend admission intentionally held by the acceptance test"
                .to_owned()
                .try_into()?,
        },
        channels: vec![ChannelDescription::NativeCodex {
            transport: NativeCarrier::UnixWebSocket,
            path: "codex-native.sock".to_owned().try_into()?,
            schema_digest: Some(proof.schemas.schema_digest().to_owned().try_into()?),
            generation: None,
        }],
    };
    let gate = NativeGenerationGate::default();
    let identity = ServiceIdentity::new(
        &service_id,
        &epoch,
        &String::from(proof.client.identity().control_schema_digest.clone()),
    )?
    .with_endpoints(vec![description.clone()])?
    .with_automation_store(Arc::clone(&store))
    .with_native_backend(NativeControlBackend {
        endpoint,
        gate: gate.clone(),
        codex_home: std::path::PathBuf::from(std::env::var_os("HOME").ok_or("HOME missing")?)
            .join(".codex"),
    })?;
    let mut connections = tokio::task::JoinSet::new();
    let mut client = connect_control(&identity, &mut connections).await?;
    let imported = client
        .import_schedule(ScheduleImportRequest {
            operation_id: OperationId::generate(),
            package_utf8: portable.package_utf8,
            overwrite: false,
        })
        .await?;
    if imported.schedule_id != portable.schedule_id
        || imported.change_id == portable.source_change_id
        || imported.definition.enabled
        || !matches!(
            imported.definition.destination,
            ExecutionDestination::FreshEachRunUnprepared
        )
        || !matches!(&imported.imported_continuity, ImportedContinuity::ImportedSummary { text, .. } if text == &portable.summary_text)
    {
        return Err("Portable import did not preserve identity, summary and fresh execution mode with a new edit token and disabled unprepared bindings".into());
    }
    let runs = client
        .list_runs(RunListRequest {
            schedule_id: imported.schedule_id.clone(),
            cursor: None,
            limit: 100.try_into()?,
        })
        .await?;
    if !runs.records.is_empty() || runs.next_cursor.is_some() || gate.acquire().is_ok() {
        return Err("Import created execution or unexpectedly opened native admission".into());
    }
    proof.record("disabledImportWithRealSummaryVerified", json!(imported))?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let _cancel_on_drop = shutdown.clone().drop_guard();
    let worker = tokio::spawn(
        identity
            .wake_timing_worker()
            .ok_or("Wake worker missing")?
            .run(shutdown.clone()),
    );
    let marker = format!("DELIVERY_RECOVERED_{}", OperationId::generate().as_str());
    let request = WakeSendRequest {
        operation_id: OperationId::generate(),
        message: SavedMessage {
            target: target.clone(),
            content: MessageContent::Agent {
                sender: target.clone(),
                text: format!("Output exactly {marker}. Do not call tools or spawn agents.")
                    .try_into()?,
            },
            delivery: MessageDelivery::Auto,
            generation_guard: None,
        },
        timing: TimingRequest::After {
            seconds: 1.try_into()?,
        },
        expiry: ExpiryRequest::None,
    };
    let wake = client.send_wakeup(request.clone()).await?;
    let waiter = connect_control(&identity, &mut connections)
        .await?
        .subscribe_wakeup(WakeShowRequest {
            wakeup_id: wake.definition.wakeup_id.clone(),
        })
        .await?;
    let fire =
        tokio::time::timeout(Duration::from_secs(10), waiter.wait_until_first_fire()).await??;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let pending = loop {
        interval.tick().await;
        let deliveries = client
            .list_deliveries(DeliveryListRequest {
                wakeup_id: Some(wake.definition.wakeup_id.clone()),
                cursor: None,
                limit: 100.try_into()?,
            })
            .await?;
        if deliveries.records.len() > 1 || deliveries.next_cursor.is_some() {
            return Err("One firing created duplicate delivery obligations".into());
        }
        if let Some(delivery) = deliveries.records.into_iter().next() {
            match &delivery.evidence {
                DeliveryEvidence::KnownNotSubmitted { .. } => break delivery,
                DeliveryEvidence::Accepted { .. } | DeliveryEvidence::OutcomeUnknown { .. } => {
                    return Err(
                        "Closed native admission unexpectedly acquired external effects".into(),
                    );
                }
                _ => {}
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "No known non-submission evidence while backend admission was closed".into(),
            );
        }
    };
    if gate.acquire().is_ok() || fire.wakeup_id != wake.definition.wakeup_id {
        return Err("First-fire proof did not occur under closed backend admission".into());
    }
    proof.record(
        "firstFireBeforeNativeAcceptanceVerified",
        json!({"fire":fire,"delivery":pending}),
    )?;
    gate.activate(
        generation.clone(),
        proof.root.join("native-socket/app-server.sock"),
        Some(Arc::clone(&proof.schemas)),
    )?;
    description.availability = EndpointAvailability::Available {
        observed_at: timestamp()?,
    };
    for channel in &mut description.channels {
        if let ChannelDescription::NativeCodex {
            generation: published,
            ..
        } = channel
        {
            *published = Some(generation.clone());
        }
    }
    identity
        .endpoint_directory()
        .publish(description)
        .map_err(|error| error.to_string())?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let accepted = loop {
        interval.tick().await;
        let delivery = client
            .read_delivery(DeliveryShowRequest {
                delivery_id: pending.delivery_id.clone(),
            })
            .await?;
        if matches!(delivery.evidence, DeliveryEvidence::Accepted { .. }) {
            break delivery;
        }
        if matches!(delivery.evidence, DeliveryEvidence::OutcomeUnknown { .. }) {
            return Err("Recovered backend delivery is uncertain; input must not be resent".into());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "Same delivery did not obtain native acceptance after admission opened".into(),
            );
        }
    };
    let turns = proof.wait_for_text(&target, &marker).await?;
    let count = turns
        .iter()
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("userMessage")
                && item.get("clientId").and_then(Value::as_str)
                    == Some(pending.delivery_id.as_str())
        })
        .count();
    if count != 1 {
        return Err(
            "Recovered delivery did not produce exactly one correlated native input".into(),
        );
    }
    proof.record(
        "sameDeliveryRecoveredToRealLuna",
        json!({"fire":fire,"delivery":accepted,"nativeInputCount":count}),
    )?;
    let mut paused_request = request;
    paused_request.operation_id = OperationId::generate();
    paused_request.timing = TimingRequest::After {
        seconds: 60.try_into()?,
    };
    let paused = client.send_wakeup(paused_request).await?;
    let waiter = connect_control(&identity, &mut connections)
        .await?
        .subscribe_wakeup(WakeShowRequest {
            wakeup_id: paused.definition.wakeup_id.clone(),
        })
        .await?;
    client
        .pause_wakeup(WakeMutationRequest {
            operation_id: OperationId::generate(),
            wakeup_id: paused.definition.wakeup_id.clone(),
        })
        .await?;
    if !matches!(tokio::time::timeout(Duration::from_secs(5), waiter.wait_until_first_fire()).await?,
        Err(WakeWaitError::Paused { wakeup_id }) if wakeup_id == paused.definition.wakeup_id)
    {
        return Err("Pause before first firing did not produce a typed wait error".into());
    }
    client
        .cancel_wakeup(WakeMutationRequest {
            operation_id: OperationId::generate(),
            wakeup_id: paused.definition.wakeup_id.clone(),
        })
        .await?;
    proof.record(
        "pausedFirstFireWaitRejected",
        json!({"wakeupId":paused.definition.wakeup_id}),
    )?;
    client.close().await?;
    shutdown.cancel();
    worker.await?;
    while let Some(connection) = connections.join_next().await {
        connection??;
    }
    Ok(())
}

async fn connect_control(
    identity: &ServiceIdentity,
    tasks: &mut tokio::task::JoinSet<Result<(), String>>,
) -> ProofResult<ControlClient> {
    let (client, server) = tokio::net::UnixStream::pair()?;
    let identity = identity.clone();
    tasks.spawn(async move {
        communication_service::serve_control_connection(server, identity)
            .await
            .map_err(|error| error.to_string())
    });
    Ok(ControlClient::initialize(client, "durable-recovery-proof", "1").await?)
}

fn timestamp() -> ProofResult<communication_protocol::ObservationTimestamp> {
    Ok(chrono::Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        .try_into()?)
}
