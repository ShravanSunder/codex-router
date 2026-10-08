use collaboration_service::{
    BoardAvailability, MachineIdentity, ServiceIdentity, SessionDeliveryRouter,
    SessionMessageDelivery, SubscriptionDeliveryService, SubscriptionDeliveryServiceProps,
    SystemSubscriptionClock, TargetPresenceProbe,
};
use serde_json::json;
use std::sync::Arc;
#[path = "support/served_api.rs"]
mod served_api;

#[tokio::test]
async fn message_to_missing_endpoint_reports_no_native_effects() {
    // Arrange: the served API without a backend.
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
    let served = served_api::ServedApi::start(identity).await.unwrap();
    let target =
        json!({"endpoint":{"serviceId":id,"endpointId":"codex-local"},"sessionId":"thread"});
    // Act: submit through the actual tool, not an error helper.
    let last = served
        .call(
            "message_send",
            json!({"target":target,"generationGuard":{"serviceEpoch":id,"generation":1},"message":{"kind":"agent","sender":target,"text":"information"}}),
        )
        .await
        .unwrap();
    served.stop().await.unwrap();
    subscription_delivery.shutdown().await;
    drop(subscription_delivery);
    Arc::try_unwrap(automation_store)
        .unwrap_or_else(|_| panic!("service retained automation store"))
        .into_inner()
        .close()
        .await
        .unwrap();
    // Assert: the tool exists and its rejection truthfully establishes no effects. The API
    // publishes a rejected delivery as a tool error carrying the stored push's receipt.
    let receipt = &last["error"]["data"]["receipt"];
    assert_eq!(receipt["outcome"]["kind"], "rejected", "{last}");
    assert_eq!(receipt["outcome"]["reason"], "noRoute");
    assert!(receipt["reachability"].is_null());
    assert!(receipt["client"].is_null());
    assert_eq!(last["error"]["data"]["effect"], "none");
}
