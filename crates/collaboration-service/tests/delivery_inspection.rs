//! A missing delivery is named, with the command that shows a push record instead.
use automation_storage::AutomationStore;
use collaboration_protocol::{DeliveryShowRequest, WakeFailureReason};
use collaboration_service::{CollaborationApplication, ServiceIdentity};
use std::sync::Arc;

#[tokio::test]
async fn missing_delivery_show_names_the_id_and_push_show_command() {
    // Arrange
    let directory = tempfile::tempdir().expect("isolated automation store");
    let store = AutomationStore::open(&directory.path().join("automation.sqlite"))
        .await
        .expect("automation store");
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
    )
    .expect("service identity")
    .with_automation_store(Arc::new(tokio::sync::Mutex::new(store)));
    let application = CollaborationApplication::new(identity);
    let delivery_id = agent_automation::DeliveryId::generate();

    // Act
    let failure = application
        .wakes()
        .delivery_show(DeliveryShowRequest {
            delivery_id: delivery_id.clone(),
        })
        .await
        .expect_err("an unknown delivery is not found");

    // Assert
    assert!(matches!(
        failure.reason,
        WakeFailureReason::ResourceNotFound
    ));
    assert_eq!(
        failure.message,
        format!(
            "Delivery {} was not found; for a push record, run agent-collaboration show <link>.",
            delivery_id.as_str()
        )
    );
}
