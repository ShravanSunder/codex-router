use std::sync::Arc;

use collaboration_client::ControlClient;
use collaboration_protocol::{
    EndpointDescription, NativeSessionScope, NativeSessionSource, NativeSessionView,
    ProviderRequestedPolicy, ProviderSessionListParams, ProviderSessionState,
    ProviderWorkingDirectory, RouterAccess, SessionRef,
};
use collaboration_service::{
    ProviderOperationStore, ProviderSessionEventHub, ProviderSessionRecord, ServiceIdentity,
    serve_control_connection,
};
use serde_json::json;
use tokio::sync::Mutex;

#[tokio::test]
async fn provider_inventory_control_reads_durable_rows_with_hub_state() {
    let root = tempfile::tempdir().expect("service directory");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"claude-local"},
        "sessionId":"provider-session"
    }))
    .expect("target");
    let creator: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"creator"
    }))
    .expect("creator");
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
            created_by: creator.clone(),
            approver: creator.clone(),
            updated_at_ms: 3_000,
        })
        .await
        .expect("record session");
    let older: SessionRef = serde_json::from_value(json!({
        "endpoint":target.endpoint,"sessionId":"older-session"
    }))
    .expect("older target");
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: older.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: creator.clone(),
            approver: creator.clone(),
            updated_at_ms: 2_000,
        })
        .await
        .expect("record older session");
    let hub = Arc::new(ProviderSessionEventHub::new(store.clone()));
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Claude fixture",
        "availability":{"state":"available","observedAt":"2026-09-26T00:00:00Z"},
        "channels":[{"kind":"externalProvider","transport":"stdioAcp",
            "bindingId":"fixture-binding","bindingGeneration":7,
            "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
            "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
    }))
    .expect("provider endpoint");
    let no_channel: EndpointDescription = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"unbound-provider"},
        "label":"Unbound fixture",
        "availability":{"state":"unavailable","observedAt":"2026-09-26T00:00:00Z",
            "reason":"provider not configured"},
        "channels":[]
    }))
    .expect("endpoint without provider channel");
    let identity = ServiceIdentity::new(service_id, epoch, &format!("sha256:{}", "a".repeat(64)))
        .expect("identity")
        .with_endpoints(vec![description, no_channel])
        .expect("endpoint")
        .with_provider_operation_store(store)
        .with_provider_session_hub(hub.clone());
    let second_identity = identity.clone();
    let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("socket");
    let server = tokio::spawn(serve_control_connection(server_stream, identity));
    let mut client = ControlClient::initialize(client_stream, "provider-list-test", "1")
        .await
        .expect("initialize");
    let params = ProviderSessionListParams {
        endpoint: target.endpoint.clone(),
        view: NativeSessionView::Stored,
        scope: NativeSessionScope::Any,
        source: NativeSessionSource::All,
        query: None,
        page_size: 1,
        cursor: None,
    };
    let cold = client
        .list_provider_sessions(params.clone())
        .await
        .expect("cold list");
    assert_eq!(cold.sessions.len(), 1);
    let next_cursor = cold.next_cursor.clone().expect("next page");
    assert_eq!(cold.sessions[0].target, target);
    assert_eq!(cold.sessions[0].state, ProviderSessionState::Unloaded);
    assert_eq!(cold.sessions[0].updated_at, 3);
    assert_eq!(cold.sessions[0].created_by, creator);
    let wire = serde_json::to_value(&cold.sessions[0]).expect("row JSON");
    assert_eq!(
        wire["approver"],
        json!({"kind":"session","session":creator})
    );
    assert!(wire.get("title").is_none());
    let older_page = client
        .list_provider_sessions(ProviderSessionListParams {
            cursor: Some(next_cursor),
            ..params.clone()
        })
        .await
        .expect("older page");
    assert_eq!(older_page.sessions.len(), 1);
    assert_eq!(older_page.sessions[0].target, older);
    assert!(older_page.next_cursor.is_none());
    let hub_target = serde_json::from_value(serde_json::to_value(&target).expect("target JSON"))
        .expect("hub target");
    hub.publish(
        hub_target,
        session_event_model::SessionEvent::TurnStarted {
            turn_id: "turn-1".into(),
            input_id: session_event_model::InputId::new("input-1").expect("input ID"),
        },
    )
    .await
    .expect("publish");
    let live = client
        .list_provider_sessions(params.clone())
        .await
        .expect("live list");
    assert_eq!(live.sessions[0].state, ProviderSessionState::Running);
    let invalid_source = client
        .list_provider_sessions(ProviderSessionListParams {
            source: NativeSessionSource::Interactive,
            ..params
        })
        .await
        .expect_err("provider source is unknown");
    assert!(matches!(
        invalid_source,
        collaboration_client::ClientError::Rejected { code: -32602, .. }
    ));
    client.close().await.expect("close");
    server.await.expect("server task").expect("serve");

    let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("second socket");
    let server = tokio::spawn(serve_control_connection(server_stream, second_identity));
    let mut client = ControlClient::initialize(client_stream, "provider-list-test", "1")
        .await
        .expect("second initialize");
    let wrong_channel = client
        .list_provider_sessions(ProviderSessionListParams {
            endpoint: serde_json::from_value(json!({
                "serviceId":service_id,"endpointId":"unbound-provider"
            }))
            .expect("unbound endpoint"),
            ..ProviderSessionListParams {
                endpoint: target.endpoint.clone(),
                view: NativeSessionView::Stored,
                scope: NativeSessionScope::Any,
                source: NativeSessionSource::All,
                query: None,
                page_size: 1,
                cursor: None,
            }
        })
        .await
        .expect_err("an endpoint without a provider channel is unsupported");
    assert!(
        matches!(
            &wrong_channel,
            collaboration_client::ClientError::Rejected { code: -32050, .. }
        ),
        "{wrong_channel:?}"
    );
    client.close().await.expect("second close");
    server
        .await
        .expect("second server task")
        .expect("second serve");
}
