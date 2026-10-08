use collaboration_service::{
    BoardAvailability, MachineIdentity, ServiceIdentity, SessionDeliveryRouter,
    SessionMessageDelivery, SubscriptionDeliveryService, SubscriptionDeliveryServiceProps,
    SystemSubscriptionClock, TargetPresenceProbe, serve_control_connection,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn message_to_missing_endpoint_reports_no_native_effects() {
    // Arrange: an initialized real Control socket without a backend.
    let id = "00000000-0000-4000-8000-000000000001";
    let directory = tempfile::tempdir().unwrap();
    let automation_store = Arc::new(tokio::sync::Mutex::new(
        automation_storage::AutomationStore::open(&directory.path().join("automation.sqlite"))
            .await
            .unwrap(),
    ));
    let delivery_router = Arc::new(SessionDeliveryRouter::new(vec![]));
    let delivery: Arc<dyn SessionMessageDelivery> = delivery_router.clone();
    let presence: Arc<dyn TargetPresenceProbe> = delivery_router;
    let machine_identity = MachineIdentity::new(id.to_owned().try_into().unwrap(), None).unwrap();
    let subscription_delivery =
        SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
            board_availability: BoardAvailability::Unavailable,
            push_store: Arc::clone(&automation_store),
            delivery: Arc::clone(&delivery),
            presence: Arc::clone(&presence),
            machine_identity,
            clock: Arc::new(SystemSubscriptionClock),
        });
    subscription_delivery.start().await.unwrap();
    let identity = ServiceIdentity::new(id, id)
        .unwrap()
        .with_automation_store(Arc::clone(&automation_store))
        .with_session_delivery(delivery)
        .with_subscription_delivery_service(subscription_delivery.clone(), presence);
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let task = tokio::spawn(serve_control_connection(server, identity));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    let target =
        json!({"endpoint":{"serviceId":id,"endpointId":"codex-local"},"sessionId":"thread"});
    let requests = [
        json!({"jsonrpc":"2.0","id":"init","method":"control/initialize","params":{"version":{"major":1,"minor":0},"client":{"name":"fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","id":"send","method":"message/send","params":{"target":target,"generationGuard":{"serviceEpoch":id,"generation":1},"message":{"kind":"agent","sender":target,"text":"information"}}}),
    ];
    // Act: submit through the actual dispatcher, not an error helper.
    let mut last = Value::Null;
    for request in requests {
        writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        last = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    }
    writer.shutdown().await.unwrap();
    task.await.unwrap().unwrap();
    subscription_delivery.shutdown().await;
    drop(subscription_delivery);
    Arc::try_unwrap(automation_store)
        .unwrap_or_else(|_| panic!("service retained automation store"))
        .into_inner()
        .close()
        .await
        .unwrap();
    // Assert: method exists and rejection truthfully establishes no effects.
    assert_eq!(last["result"]["receipt"]["outcome"]["kind"], "rejected");
    assert_eq!(last["result"]["receipt"]["outcome"]["reason"], "noRoute");
    assert!(last["result"]["receipt"]["reachability"].is_null());
    assert!(last["result"]["receipt"]["client"].is_null());
}
