use agent_automation::{ExpiryRule, TimingRule};
use automation_storage::{
    AutomationStore, DeliveryCompletion, DeliveryResult, WakeCreate, WakeEvaluation,
};
use collaboration_client::ControlClient;
use collaboration_protocol::{
    AutomationEventDetails, AutomationEventsRequest, DeliveryAttemptsRequest, DeliveryEvidence,
    DeliveryReceipt, OperationId, SavedMessage, SessionRef,
};
use collaboration_service::{ServiceIdentity, serve_control_connection};
use serde_json::json;
use sqlx::Connection;
use std::sync::Arc;

#[tokio::test]
async fn retried_wake_retains_both_receipts_in_attempt_and_event_history()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "historical-wake-receipts-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(AutomationStore::open(&path).await?));
    let service_id = "00000000-0000-4000-8000-000000000001";
    let identity = ServiceIdentity::new(
        service_id,
        service_id,
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(Arc::clone(&store));
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let service = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "history-fixture", "1").await?;
    let target = json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"fixture-thread"});
    let message: SavedMessage = serde_json::from_value(json!({
        "target":target,"content":{"kind":"agent","sender":target,"text":"Check status"},
        "delivery":"auto","generationGuard":null
    }))?;
    let anchor_ms = chrono::Utc::now().timestamp_millis() - 5_000;
    let wake = store
        .lock()
        .await
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message,
            timing: TimingRule::After { seconds: 1 },
            expiry: ExpiryRule::None,
            now_ms: anchor_ms,
        })
        .await?;
    let now_ms = anchor_ms + 2_000;
    let WakeEvaluation::Fired { delivery_id, .. } = store
        .lock()
        .await
        .evaluate_wakeup::<SavedMessage>(&wake.definition.wakeup_id, now_ms)
        .await?
    else {
        return Err("wake did not fire".into());
    };
    let first = store.lock().await
        .claim_delivery::<SessionRef, collaboration_protocol::MessageContent, collaboration_protocol::CodexGeneration>(&delivery_id, now_ms)
        .await?.ok_or("first attempt missing")?;
    let first_receipt: DeliveryReceipt = serde_json::from_value(json!({
        "outcome":{"kind":"notSubmitted","retryable":true,"reason":"provider starting"},
        "reachability":null,"client":null
    }))?;
    store
        .lock()
        .await
        .complete_delivery(DeliveryCompletion::<
            SessionRef,
            collaboration_protocol::CodexGeneration,
            DeliveryReceipt,
        > {
            delivery_id: delivery_id.clone(),
            attempt_id: first.attempt_id.clone(),
            effects: None,
            result: DeliveryResult::KnownNotSubmitted {
                reason: "provider starting".into(),
                retryable: true,
                receipt: Some(first_receipt.clone()),
            },
            now_ms,
        })
        .await?;
    let second_at = now_ms + 2_000;
    let second = store.lock().await
        .claim_delivery::<SessionRef, collaboration_protocol::MessageContent, collaboration_protocol::CodexGeneration>(&delivery_id, second_at)
        .await?.ok_or("second attempt missing")?;
    let claimed = store
        .lock()
        .await
        .read_delivery::<SessionRef, collaboration_protocol::CodexGeneration, DeliveryReceipt>(
            &delivery_id,
        )
        .await?;
    if claimed.receipt.is_some() {
        return Err("new attempt inherited the previous receipt".into());
    }
    let second_receipt: DeliveryReceipt = serde_json::from_value(json!({
        "outcome":{"kind":"rejected","reason":"noRoute","nextAction":"correctRequest","clientCode":null,"detail":"no route"},
        "reachability":null,"client":null
    }))?;
    store
        .lock()
        .await
        .complete_delivery(DeliveryCompletion::<
            SessionRef,
            collaboration_protocol::CodexGeneration,
            DeliveryReceipt,
        > {
            delivery_id: delivery_id.clone(),
            attempt_id: second.attempt_id.clone(),
            effects: None,
            result: DeliveryResult::KnownNotSubmitted {
                reason: "no route".into(),
                retryable: false,
                receipt: Some(second_receipt.clone()),
            },
            now_ms: second_at,
        })
        .await?;

    let attempts = client
        .read_delivery_attempts(DeliveryAttemptsRequest {
            delivery_id: delivery_id.clone(),
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    assert_receipt(&attempts.records, &first.attempt_id, &first_receipt)?;
    assert_receipt(&attempts.records, &second.attempt_id, &second_receipt)?;

    let events = client
        .read_automation_events(AutomationEventsRequest {
            after: None,
            limit: 100.try_into()?,
        })
        .await?;
    let first_event = events.records.iter().find(|event| event.change == "attemptCompleted" && matches!(
        &event.details, AutomationEventDetails::DeliveryAttempt { attempt } if attempt.attempt_id == first.attempt_id
    )).ok_or("first completion event missing")?;
    let second_event = events.records.iter().find(|event| event.change == "attemptCompleted" && matches!(
        &event.details, AutomationEventDetails::DeliveryAttempt { attempt } if attempt.attempt_id == second.attempt_id
    )).ok_or("second completion event missing")?;
    assert_event_receipt(first_event, &first_receipt)?;
    assert_event_receipt(second_event, &second_receipt)?;

    // An old archived event has no receipt member. It must still project as known absence.
    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await?;
    let changed = sqlx::query("UPDATE automation_events SET event_body_json=json_remove(event_body_json,'$.receipt') WHERE subject_id=? AND event_kind='attemptArchived' AND json_extract(event_body_json,'$.attempt.attemptId')=?")
        .bind(delivery_id.as_str()).bind(first.attempt_id.as_str())
        .execute(&mut connection).await?.rows_affected();
    if changed != 1 {
        return Err("legacy archive fixture missing".into());
    }
    connection.close().await?;
    let legacy_attempts = client
        .read_delivery_attempts(DeliveryAttemptsRequest {
            delivery_id,
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    let old = legacy_attempts
        .records
        .iter()
        .find(|attempt| attempt.attempt_id == first.attempt_id)
        .ok_or("legacy attempt missing")?;
    if !matches!(
        old.evidence,
        DeliveryEvidence::KnownNotSubmitted { receipt: None, .. }
    ) {
        return Err("legacy archive invented a receipt".into());
    }
    assert_receipt(
        &legacy_attempts.records,
        &second.attempt_id,
        &second_receipt,
    )?;
    let legacy_events = client
        .read_automation_events(AutomationEventsRequest {
            after: None,
            limit: 100.try_into()?,
        })
        .await?;
    let archived = legacy_events.records.iter().find(|event| event.change == "attemptArchived" && matches!(
        &event.details, AutomationEventDetails::DeliveryAttempt { attempt } if attempt.attempt_id == first.attempt_id
    )).ok_or("legacy archived event missing")?;
    let AutomationEventDetails::DeliveryAttempt { attempt } = &archived.details else {
        return Err("legacy archive has wrong event type".into());
    };
    if !matches!(
        attempt.evidence,
        DeliveryEvidence::KnownNotSubmitted { receipt: None, .. }
    ) {
        return Err("legacy event invented a receipt".into());
    }
    client.close().await?;
    service.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}

fn assert_receipt(
    attempts: &[collaboration_protocol::AttemptInspection],
    attempt_id: &agent_automation::AttemptId,
    expected: &DeliveryReceipt,
) -> Result<(), Box<dyn std::error::Error>> {
    let record = attempts
        .iter()
        .find(|attempt| &attempt.attempt_id == attempt_id)
        .ok_or("attempt missing")?;
    let actual = match &record.evidence {
        DeliveryEvidence::KnownNotSubmitted {
            receipt: Some(receipt),
            ..
        } => receipt,
        _ => return Err("completed attempt receipt missing".into()),
    };
    if serde_json::to_value(actual)? != serde_json::to_value(expected)? {
        return Err("completed attempt receipt changed".into());
    }
    Ok(())
}

fn assert_event_receipt(
    event: &collaboration_protocol::AutomationEvent,
    expected: &DeliveryReceipt,
) -> Result<(), Box<dyn std::error::Error>> {
    let AutomationEventDetails::DeliveryAttempt { attempt } = &event.details else {
        return Err("event was not an attempt".into());
    };
    assert_receipt(
        std::slice::from_ref(attempt.as_ref()),
        &attempt.attempt_id,
        expected,
    )
}
