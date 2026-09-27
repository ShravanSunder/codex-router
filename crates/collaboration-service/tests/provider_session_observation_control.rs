//! Provider observers attach through Control to one ordered Session hub.

use std::sync::Arc;

use collaboration_client::ControlClient;
use collaboration_protocol::{
    BoundedObservationRequest, EndpointDescription, ObservationEndReason, ProviderRequestedPolicy,
    ProviderSessionListenRequest, ProviderWorkingDirectory, RouterAccess, SessionRef,
};
use collaboration_service::{
    ProviderOperationStore, ProviderSessionEventHub, ProviderSessionRecord, ServiceIdentity,
    serve_control_connection,
};
use serde_json::{Value, json};
use session_event_model::{SessionEvent, SessionItem, SessionItemKind};
use tokio::sync::Mutex;

#[tokio::test]
async fn provider_observers_share_snapshot_and_live_order() {
    let root = tempfile::tempdir().expect("service root");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"claude-local"},
        "sessionId":"provider-session"
    }))
    .expect("provider target");
    let owner: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"owner"
    }))
    .expect("owner");
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: owner.clone().into(),
            approver: owner.into(),
            updated_at_ms: 1,
        })
        .await
        .expect("session row");
    let hub = Arc::new(ProviderSessionEventHub::with_capacity(store, 2));
    let board_target: message_board::SessionRef =
        serde_json::from_value(serde_json::to_value(&target).expect("target JSON"))
            .expect("board target");
    hub.publish(board_target.clone(), item("item-1", "first"))
        .await
        .expect("first item");
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Claude fixture",
        "availability":{"state":"available","observedAt":"2026-09-26T00:00:00Z"},
        "channels":[{"kind":"externalProvider","transport":"stdioAcp",
            "bindingId":"fixture-binding","bindingGeneration":7,
            "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
            "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
    }))
    .expect("endpoint");
    let identity = ServiceIdentity::new(service_id, epoch, &format!("sha256:{}", "a".repeat(64)))
        .expect("identity")
        .with_provider_session_hub(hub.clone());
    identity
        .endpoint_directory()
        .publish(description)
        .expect("endpoint publication");

    let (mut first, first_server) = client(&identity).await.expect("first client");
    let (mut second, second_server) = client(&identity).await.expect("second client");
    let first_ready = first
        .listen_provider_session(ProviderSessionListenRequest {
            target: target.clone(),
        })
        .await
        .expect("first attach");
    let second_ready = second
        .listen_provider_session(ProviderSessionListenRequest {
            target: target.clone(),
        })
        .await
        .expect("second attach");
    assert_eq!(first_ready.generation, second_ready.generation);
    assert_eq!(u64::from(first_ready.generation.generation), 7);
    let first_snapshot = next(&mut first).await.expect("first snapshot");
    assert_eq!(
        first_snapshot,
        next(&mut second).await.expect("second snapshot")
    );
    assert_eq!(first_snapshot["sequence"], 1);

    hub.publish(board_target, item("item-2", "second"))
        .await
        .expect("live item");
    let first_live = next(&mut first).await.expect("first live event");
    assert_eq!(
        first_live,
        next(&mut second).await.expect("second live event")
    );
    assert_eq!(first_live["sequence"], 2);

    let (mut bounded, bounded_server) = client(&identity).await.expect("bounded client");
    let observed = bounded
        .observe_provider_session(BoundedObservationRequest {
            target,
            timeout_seconds: 1,
            max_events: 2,
            max_bytes: 262_144,
        })
        .await
        .expect("bounded observation");
    assert_eq!(observed.events, vec![first_snapshot, first_live]);
    assert_eq!(
        observed.end_reason,
        ObservationEndReason::ResultLimitReached
    );
    assert!(observed.continuation_gap);
    for index in 3..=5 {
        hub.publish(
            serde_json::from_value(serde_json::to_value(&observed.target).expect("target JSON"))
                .expect("board target"),
            item(&format!("item-{index}"), "overflow subscriber buffer"),
        )
        .await
        .expect("overflow event");
    }
    assert_eq!(
        next(&mut first).await.expect("first lag"),
        json!({"kind":"resyncRequired"})
    );
    assert_eq!(
        next(&mut second).await.expect("second lag"),
        json!({"kind":"resyncRequired"})
    );
    first.close().await.expect("first close");
    second.close().await.expect("second close");
    bounded.close().await.expect("bounded close");
    first_server
        .await
        .expect("first service")
        .expect("first service IO");
    second_server
        .await
        .expect("second service")
        .expect("second service IO");
    bounded_server
        .await
        .expect("bounded service")
        .expect("bounded service IO");
}

fn item(item_id: &str, text: &str) -> SessionEvent {
    SessionEvent::ItemStarted {
        item: SessionItem {
            item_id: item_id.into(),
            kind: SessionItemKind::AgentMessage,
            text: Some(text.into()),
        },
    }
}

async fn client(
    identity: &ServiceIdentity,
) -> Result<(ControlClient, tokio::task::JoinHandle<std::io::Result<()>>), Box<dyn std::error::Error>>
{
    let (client_stream, server_stream) = tokio::net::UnixStream::pair()?;
    let server = tokio::spawn(serve_control_connection(server_stream, identity.clone()));
    let client = ControlClient::initialize(client_stream, "provider-observer", "1").await?;
    Ok((client, server))
}

async fn next(client: &mut ControlClient) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.next_provider_session_notification(),
    )
    .await??)
}
