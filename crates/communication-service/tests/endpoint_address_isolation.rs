//! Public address readers preserve endpoint identity when backend-local IDs collide.
#[cfg(test)]
mod tests {
    use communication_client::ControlClient;
    use communication_protocol::{
        ArchiveState, EndpointDescription, EndpointRef, LifecycleObservation,
    };
    use communication_service::{ServiceIdentity, serve_control_connection};
    use lifecycle_observation::{LifecycleStore, ObservationJournal};
    use serde_json::json;
    use std::sync::Arc;

    const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
    const EPOCH: &str = "00000000-0000-4000-8000-000000000002";

    #[tokio::test]
    async fn colliding_native_ids_remain_distinct_through_public_discovery_and_address_reads() {
        // Arrange: two logical, unavailable endpoints with historical observations.
        // This fixture does not introduce a second production backend or remote carrier.
        let path =
            std::env::temp_dir().join(format!("endpoint-addresses-{}.sqlite", std::process::id()));
        assert!(!path.exists());
        let journal = ObservationJournal::open(&path, SERVICE.to_owned().try_into().unwrap())
            .await
            .unwrap();
        let store = Arc::new(LifecycleStore::new(journal));
        let first = endpoint("first-codex");
        let second = endpoint("second-codex");
        for endpoint in [&first, &second] {
            let observation: LifecycleObservation = serde_json::from_value(json!({
                "observedAt":"2026-09-06T12:00:00Z","source":"inventoryRead",
                "scope":{"endpoint":endpoint,"generation":null,"observerId":EPOCH},
                "subject":{"kind":"thread","address":{"endpoint":endpoint,"nativeThreadId":"shared-native-id"}},
                "change":{"kind":"threadDiscovered"}
            })).unwrap();
            store.append(&observation, 100).await.unwrap();
        }
        let archived: LifecycleObservation = serde_json::from_value(json!({
            "observedAt":"2026-09-06T12:01:00Z","source":"nativeNotification",
            "scope":{"endpoint":second,"generation":{"serviceEpoch":EPOCH,"generation":1},"observerId":EPOCH},
            "subject":{"kind":"thread","address":{"endpoint":second,"nativeThreadId":"shared-native-id"}},
            "change":{"kind":"threadArchived"}
        })).unwrap();
        store.append(&archived, 101).await.unwrap();
        let identity = ServiceIdentity::new(SERVICE, EPOCH, &format!("sha256:{}", "a".repeat(64)))
            .unwrap()
            .with_endpoints(vec![
                description(&first, "first.sock"),
                description(&second, "second.sock"),
            ])
            .unwrap()
            .with_journal(Arc::clone(&store));
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let service = tokio::spawn(serve_control_connection(server, identity));
        let mut client = ControlClient::initialize(client, "address-isolation", "1")
            .await
            .unwrap();

        // Act: resolve each explicit endpoint through the actual public Rust client.
        let inventory = client.list_endpoints().await.unwrap();
        let first_page = client.list_addresses(&first, 100, None).await.unwrap();
        let second_page = client.list_addresses(&second, 100, None).await.unwrap();

        // Assert: equal local IDs neither merge rows nor transfer lifecycle facts.
        assert_eq!(inventory.endpoints.len(), 2);
        assert!(inventory.endpoints.iter().any(|row| row.endpoint == first));
        assert!(inventory.endpoints.iter().any(|row| row.endpoint == second));
        assert_eq!(first_page.entries.len(), 1);
        assert_eq!(second_page.entries.len(), 1);
        assert_eq!(first_page.entries[0].address.endpoint, first);
        assert_eq!(second_page.entries[0].address.endpoint, second);
        assert_eq!(
            first_page.entries[0].address.native_thread_id,
            second_page.entries[0].address.native_thread_id
        );
        assert_ne!(
            first_page.entries[0].address,
            second_page.entries[0].address
        );
        assert!(matches!(
            first_page.entries[0].disposition.archive,
            ArchiveState::Unknown
        ));
        assert!(matches!(
            second_page.entries[0].disposition.archive,
            ArchiveState::Archived
        ));
        client.close().await.unwrap();
        service.await.unwrap().unwrap();
        Arc::try_unwrap(store)
            .unwrap_or_else(|_| panic!("store retained"))
            .close()
            .await;
        std::fs::remove_file(path).unwrap();
    }

    fn endpoint(name: &str) -> EndpointRef {
        serde_json::from_value(json!({"serviceId":SERVICE,"endpointId":name})).unwrap()
    }

    fn description(endpoint: &EndpointRef, path: &str) -> EndpointDescription {
        serde_json::from_value(json!({"endpoint":endpoint,"label":"Historical endpoint",
            "availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex",
                "transport":"unixWebSocket","path":path,"schemaDigest":null,"generation":null}]}))
        .unwrap()
    }
}
