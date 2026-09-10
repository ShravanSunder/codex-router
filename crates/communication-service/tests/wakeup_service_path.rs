use automation_storage::AutomationStore;
use communication_client::ControlClient;
use communication_protocol::{OperationId, WakeSendRequest, WakeShowRequest};
use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn real_control_client_preserves_wake_identity_timing_and_message()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "wake-rpc-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(AutomationStore::open(&path).await?));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(store.clone());
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "wake-test", "1").await?;
    let target = json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"fixture-only-thread"});
    let request: WakeSendRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"message":{"target":target.clone(),"content":{"kind":"agent","sender":target,"text":"Check status"},"delivery":"queue","generationGuard":null},"timing":{"kind":"after","seconds":60},"expiry":{"kind":"after","seconds":120}}),
    )?;
    // Actual SDK -> Unix stream -> service -> SQLite, with no native process or model.
    let first = client.send_wakeup(request.clone()).await?;
    let replay = client.send_wakeup(request).await?;
    let current = client
        .read_wakeup(WakeShowRequest {
            wakeup_id: first.definition.wakeup_id.clone(),
        })
        .await?;
    if first.definition.wakeup_id != replay.definition.wakeup_id
        || serde_json::to_value(&first.definition)? != serde_json::to_value(&current.definition)?
    {
        return Err("wake replay or inspection changed its definition".into());
    }
    if current.first_fire.is_some() || current.pending_delivery_id.is_some() {
        return Err("creation fabricated firing or native acceptance".into());
    }
    let listed = client
        .list_wakeups(communication_protocol::AutomationPageRequest {
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    if listed.records.len() != 1
        || listed
            .records
            .first()
            .is_none_or(|record| record.definition.wakeup_id != first.definition.wakeup_id)
    {
        return Err("SDK wake list omitted created wake".into());
    }
    let pause_request = communication_protocol::WakeMutationRequest {
        operation_id: OperationId::generate(),
        wakeup_id: first.definition.wakeup_id.clone(),
    };
    let paused = client.pause_wakeup(pause_request.clone()).await?;
    let replayed_pause = client.pause_wakeup(pause_request).await?;
    if serde_json::to_value(&paused)? != serde_json::to_value(replayed_pause)? {
        return Err("mutation replay changed the original receipt".into());
    }
    if paused.wakeup.state != communication_protocol::WakeState::Paused {
        return Err("pause did not reach storage".into());
    }
    let resumed = client
        .resume_wakeup(communication_protocol::WakeMutationRequest {
            operation_id: OperationId::generate(),
            wakeup_id: first.definition.wakeup_id.clone(),
        })
        .await?;
    if resumed.wakeup.state != communication_protocol::WakeState::Active
        || resumed.wakeup.definition.anchor_at != first.definition.anchor_at
    {
        return Err("resume changed original anchor".into());
    }
    let cancelled = client
        .cancel_wakeup(communication_protocol::WakeMutationRequest {
            operation_id: OperationId::generate(),
            wakeup_id: first.definition.wakeup_id,
        })
        .await?;
    if cancelled.wakeup.state != communication_protocol::WakeState::Cancelled {
        return Err("cancel did not reach storage".into());
    }
    let past = (chrono::Utc::now() - chrono::Duration::seconds(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let immediate: WakeSendRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"message":current.definition.message,"timing":{"kind":"at","at":past},"expiry":{"kind":"none"}}),
    )?;
    let immediate = client.send_wakeup(immediate).await?;
    let delivery = match store
        .lock()
        .await
        .evaluate_wakeup::<communication_protocol::SavedMessage>(
            &immediate.definition.wakeup_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?
    {
        automation_storage::WakeEvaluation::Fired { delivery_id, .. } => delivery_id,
        _ => return Err("immediate fixture wake did not fire".into()),
    };
    let empty = client
        .read_delivery_attempts(communication_protocol::DeliveryAttemptsRequest {
            delivery_id: delivery.clone(),
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    if !empty.records.is_empty() || empty.coverage.latest_attempt_included {
        return Err("empty attempt history invented submission evidence".into());
    }
    let claimed = store.lock().await.claim_delivery::<communication_protocol::SessionRef,communication_protocol::MessageContent,communication_protocol::CodexGeneration>(&delivery, chrono::Utc::now().timestamp_millis()).await?.ok_or("fixture claim missing")?;
    let attempts = client
        .read_delivery_attempts(communication_protocol::DeliveryAttemptsRequest {
            delivery_id: delivery.clone(),
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    let [attempt] = attempts.records.as_slice() else {
        return Err("expected one admitted delivery attempt".into());
    };
    if attempt.attempt_id != claimed.attempt_id
        || !attempts.coverage.latest_attempt_included
        || !matches!(
            attempt.evidence,
            communication_protocol::DeliveryEvidence::Dispatching { .. }
        )
    {
        return Err("attempt inspection lost admitted submission evidence".into());
    }
    // A scripted native acceptance exercises receipt projection through the real SDK.
    let generation = json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1});
    let receipt: communication_protocol::NativeSendReceipt = serde_json::from_value(json!({
        "target":target,"generation":generation,"inputKind":"agent",
        "representation":"declaredAgentText","clientUserMessageId":claimed.attempt_id,
        "resumeEffect":"notRequested","acceptance":{"kind":"queueAccepted",
        "submissionId":"accepted-fixture-submission"}
    }))?;
    let effects: agent_automation::NativeEffectEvidence<
        communication_protocol::SessionRef,
        communication_protocol::CodexGeneration,
    > = serde_json::from_value(json!({
        "target":target,"generation":generation,"clientUserMessageId":claimed.attempt_id,
        "nativeTurnId":null,"nativeSubmissionId":"accepted-fixture-submission",
        "allocation":"notRequested","resume":"notRequested","submission":"accepted",
        "cessation":"notApplicable"
    }))?;
    store
        .lock()
        .await
        .complete_delivery(automation_storage::DeliveryCompletion {
            delivery_id: delivery.clone(),
            attempt_id: claimed.attempt_id,
            effects,
            result: automation_storage::DeliveryResult::Accepted {
                receipt: receipt.clone(),
            },
            now_ms: chrono::Utc::now().timestamp_millis(),
        })
        .await?;
    let cancellation_request = communication_protocol::WakeMutationRequest {
        operation_id: OperationId::generate(),
        wakeup_id: immediate.definition.wakeup_id,
    };
    let cancelled = client.cancel_wakeup(cancellation_request.clone()).await?;
    let [retained] = cancelled.dispatched_deliveries.as_slice() else {
        return Err("SDK cancellation omitted previously accepted delivery".into());
    };
    if retained.delivery_id != delivery || cancelled.wakeup.pending_delivery_id.is_some() {
        return Err("SDK cancellation lost cleared-pointer delivery identity".into());
    }
    let evidence = serde_json::to_value(&retained.evidence)?;
    if evidence.get("receipt") != Some(&serde_json::to_value(receipt)?) {
        return Err("SDK cancellation changed native acceptance receipt".into());
    }
    let replayed = client.cancel_wakeup(cancellation_request).await?;
    if serde_json::to_value(&cancelled)? != serde_json::to_value(replayed)? {
        return Err("SDK cancellation replay changed evidence".into());
    }
    let changed: WakeSendRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"message":current.definition.message,"timing":{"kind":"cron","expression":"* * * * *","timezone":"invalid-zone"},"expiry":{"kind":"none"}}),
    )?;
    let error = client
        .send_wakeup(changed.clone())
        .await
        .err()
        .ok_or("invalid timezone was admitted")?;
    if !matches!(error, communication_client::WakeClientError::Rejected(_)) {
        return Err(format!("invalid timezone lacked structured guidance: {error:?}").into());
    }
    client.close().await?;
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
