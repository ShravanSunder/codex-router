//! Actual sender process loss, real recovery/store and inspection-only reconciliation.
//! Native endpoint/schema and durable queue witness stand in for upstream, not a model.
use super::*;
use agent_automation::{DeliveryStatus, PreparationEffect, RouteEffectEvidence};

#[tokio::test]
async fn generic_queue_host_stop_reconciles_exact_item_causal() {
    let fixture_id = OperationId::generate();
    let root = native_delivery_crash_root(&fixture_id);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "wakeup_delivery_sender::crash_tests::native_delivery_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env(ROOT_ENV, &root)
            .env(STAGE_ENV, "queue-before-response")
            .env(ID_ENV, fixture_id.as_str())
            .env(GENERIC_QUEUE_ENV, "true")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        output.status.code(),
        Some(CRASH_EXIT),
        "child failed before the actual generic queue boundary: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let delivery: agent_automation::DeliveryId =
        serde_json::from_slice(&std::fs::read(root.join("delivery-id.json")).unwrap()).unwrap();
    let push: PushId =
        serde_json::from_slice(&std::fs::read(root.join("push-id.json")).unwrap()).unwrap();
    let witness: Value =
        serde_json::from_slice(&std::fs::read(root.join("native-receipt.json")).unwrap()).unwrap();
    assert_eq!(witness["clientUserMessageId"], json!(push));
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite"))
            .await
            .unwrap(),
    ));
    let before=store.lock().await.read_delivery::<SessionRef,CodexGeneration,crate::stored_delivery_receipt::StoredDeliveryReceipt>(&delivery).await.unwrap();
    assert_eq!(before.status, DeliveryStatus::Dispatching);
    let attempt = before.attempt.unwrap();
    let before_attempt_id = attempt.attempt_id.clone();
    let RouteEffectEvidence::CodexAppServer(before_effects) = attempt.effects.unwrap() else {
        panic!("native effects missing");
    };
    store
        .lock()
        .await
        .recover_interrupted_deliveries(chrono::Utc::now().timestamp_millis() + 1000)
        .await
        .unwrap();
    let recovered=store.lock().await.read_delivery::<SessionRef,CodexGeneration,crate::stored_delivery_receipt::StoredDeliveryReceipt>(&delivery).await.unwrap();
    assert_eq!(recovered.status, DeliveryStatus::Uncertain);
    assert_eq!(
        recovered.attempt.as_ref().unwrap().attempt_id,
        before_attempt_id
    );
    assert!(
        store
            .lock()
            .await
            .eligible_delivery_ids(chrono::Utc::now().timestamp_millis() + 60000, 16)
            .await
            .unwrap()
            .is_empty(),
        "unknown submission must not be replayed"
    );
    // The exited child owns this abandoned socket; its process is already reaped.
    std::fs::remove_file(root.join("native.sock")).unwrap();
    let listener = tokio::net::UnixListener::bind(root.join("native.sock")).unwrap();
    let native = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let request: Value = serde_json::from_str(&text)?;
            let result = match request["method"].as_str() {
                Some("initialize") => json!({}),
                Some("initialized") => continue,
                Some("thread/queue/list") => {
                    if request.pointer("/params/threadId") != Some(&json!("fixture-thread")) {
                        return Err("reconciled unrelated target".into());
                    }
                    json!({"data":[{"id":"persisted-generic-queue","clientUserMessageId":witness["clientUserMessageId"],"input":witness["input"]}],"nextCursor":null})
                }
                _ => return Err("crash recovery replayed or resumed native input".into()),
            };
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let identity = identity(&root, store.clone(), NEW_EPOCH).unwrap();
    let delivery_route = identity.session_delivery.as_ref().unwrap();
    crate::delivery_reconciliation::reconcile(&store, delivery_route.as_ref(), recovered)
        .await
        .unwrap();
    let after=store.lock().await.read_delivery::<SessionRef,CodexGeneration,crate::stored_delivery_receipt::StoredDeliveryReceipt>(&delivery).await.unwrap();
    if after.status != DeliveryStatus::Accepted {
        native.abort();
        let _ = native.await;
        assert_eq!(
            after.status,
            DeliveryStatus::Accepted,
            "generic Queue crash must retain exact item reconciliation; recorded resume={:?}",
            before_effects.resume
        );
        return;
    }
    tokio::time::timeout(Duration::from_secs(5), native)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(before_effects.resume, PreparationEffect::NotRequested);
    assert_eq!(after.attempt.unwrap().attempt_id, before_attempt_id);
    let receipt = after.receipt.unwrap().into_public();
    assert!(matches!(
        receipt.outcome,
        collaboration_protocol::DeliveryOutcome::Queued
    ));
    let Some(collaboration_protocol::DeliveryClientReceipt::CodexAppServer(client)) =
        receipt.client
    else {
        panic!("native receipt missing");
    };
    assert_eq!(
        client.resume_effect,
        collaboration_protocol::AcceptedResumeEffect::NotRequested
    );
    assert_eq!(String::from(client.client_user_message_id), push.as_str());
    drop(identity);
    drop(store);
    for entry in std::fs::read_dir(&root).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    std::fs::remove_dir(&root).unwrap();
}
