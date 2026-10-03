use claude_code_peer_messaging::ClaudeCodeSessionRegistry;
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
    let registry_directory = root.path().join("claude-fixture-registry");
    std::fs::create_dir(&registry_directory).expect("registry directory");
    let process_id = std::process::id();
    std::fs::write(
        registry_directory.join(format!("{process_id}.json")),
        json!({
            "pid":process_id,"sessionId":"terminal-session","peerProtocol":1,
            "messagingSocketPath":root.path().join("secret-peer.sock"),
            "peerFeatures":["secret-internal-feature"],
            "cwd":root.path(),"name":"Terminal fixture","status":"waiting",
            "startedAt":1_790_162_100_123_i64,"updatedAt":1_790_162_494_441_i64,"statusUpdatedAt":1_790_162_494_789_i64,
            "kind":"interactive","entrypoint":"cli"
        })
        .to_string(),
    )
    .expect("terminal registry record");
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
            created_by: creator.clone().into(),
            approver: creator.clone().into(),
            updated_at_ms: 1_790_162_500_000,
        })
        .await
        .expect("record session");
    let older: SessionRef = serde_json::from_value(json!({
        "endpoint":target.endpoint,"sessionId":"older-session"
    }))
    .expect("older target");
    let cursor_target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"cursor-local"},
        "sessionId":"cursor-session"
    }))
    .expect("Cursor target");
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: cursor_target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: creator.clone().into(),
            approver: creator.clone().into(),
            updated_at_ms: 1_790_162_300_000,
        })
        .await
        .expect("Cursor provider record");
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
            created_by: creator.clone().into(),
            approver: creator.clone().into(),
            updated_at_ms: 1_790_162_400_000,
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
    let cursor_description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":cursor_target.endpoint,"label":"Cursor fixture",
        "availability":{"state":"available","observedAt":"2026-09-26T00:00:00Z"},
        "channels":[{"kind":"externalProvider","transport":"stdioAcp",
            "bindingId":"cursor-fixture-binding","bindingGeneration":7,
            "runtime":{"provider":"cursor","runtimeName":"fixture"},
            "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
    }))
    .expect("Cursor endpoint");
    let identity = ServiceIdentity::new(service_id, epoch, &format!("sha256:{}", "a".repeat(64)))
        .expect("identity")
        .with_endpoints(vec![description, cursor_description, no_channel])
        .expect("endpoint")
        .with_provider_operation_store(store)
        .with_provider_session_hub(hub.clone())
        .with_claude_code_sessions(Arc::new(ClaudeCodeSessionRegistry::new(registry_directory)));
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
    assert_eq!(cold.sessions[0].target(), &target);
    assert!(
        matches!(&cold.sessions[0], collaboration_protocol::ProviderSessionSummary::HostedProvider {
        state: ProviderSessionState::Unloaded, updated_at: 1_790_162_500, created_by, ..
    } if created_by == &creator.clone().into())
    );
    let wire = serde_json::to_value(&cold.sessions[0]).expect("row JSON");
    assert_eq!(wire["approver"], json!(creator));
    assert!(wire.get("title").is_none());
    let cursor_page = client
        .list_provider_sessions(ProviderSessionListParams {
            endpoint: cursor_target.endpoint.clone(),
            page_size: 10,
            ..params.clone()
        })
        .await
        .expect("Cursor provider list");
    let cursor_wire = serde_json::to_value(&cursor_page.sessions[0]).expect("Cursor row JSON");
    assert_eq!(cursor_wire["target"], json!(cursor_target));
    assert!(cursor_wire.get("origin").is_none());
    let older_page = client
        .list_provider_sessions(ProviderSessionListParams {
            cursor: Some(next_cursor),
            ..params.clone()
        })
        .await
        .expect("older page");
    assert_eq!(older_page.sessions.len(), 1);
    assert_eq!(older_page.sessions[0].target(), &older);
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
    assert!(matches!(
        &live.sessions[0],
        collaboration_protocol::ProviderSessionSummary::HostedProvider {
            state: ProviderSessionState::Running,
            ..
        }
    ));
    let terminal = client
        .list_provider_sessions(ProviderSessionListParams {
            view: NativeSessionView::Active,
            source: NativeSessionSource::Interactive,
            page_size: 10,
            ..params.clone()
        })
        .await
        .expect("Claude terminal list through Control");
    assert_eq!(terminal.sessions.len(), 1);
    assert_eq!(terminal.skipped_records, Some(0));
    assert!(matches!(
        &terminal.sessions[0],
        collaboration_protocol::ProviderSessionSummary::ClaudeCodeInteractive {
            started_at: 1_790_162_100,
            updated_at: 1_790_162_494,
            status_updated_at: Some(1_790_162_494),
            ..
        }
    ));
    let terminal_json = serde_json::to_string(&terminal).expect("terminal page JSON");
    assert!(terminal_json.contains("claudeCodeInteractive"));
    assert!(terminal_json.contains("terminal-session"));
    assert!(!terminal_json.contains("secret-peer.sock"));
    assert!(!terminal_json.contains("secret-internal-feature"));
    let both = client
        .list_provider_sessions(ProviderSessionListParams {
            view: NativeSessionView::Loaded,
            source: NativeSessionSource::All,
            page_size: 10,
            ..params.clone()
        })
        .await
        .expect("combined provider and terminal list");
    assert!(both.sessions.iter().any(|row| matches!(
        row,
        collaboration_protocol::ProviderSessionSummary::HostedProvider {
            updated_at: 1_790_162_500,
            ..
        }
    )));
    assert!(both.sessions.iter().any(|row| matches!(
        row,
        collaboration_protocol::ProviderSessionSummary::ClaudeCodeInteractive {
            updated_at: 1_790_162_494,
            ..
        }
    )));
    let first_mixed_page = client
        .list_provider_sessions(ProviderSessionListParams {
            view: NativeSessionView::Loaded,
            source: NativeSessionSource::All,
            page_size: 1,
            ..params.clone()
        })
        .await
        .expect("first mixed page");
    assert!(matches!(
        &first_mixed_page.sessions[0],
        collaboration_protocol::ProviderSessionSummary::HostedProvider {
            updated_at: 1_790_162_500,
            ..
        }
    ));
    let second_mixed_page = client
        .list_provider_sessions(ProviderSessionListParams {
            view: NativeSessionView::Loaded,
            source: NativeSessionSource::All,
            page_size: 1,
            cursor: first_mixed_page.next_cursor,
            ..params.clone()
        })
        .await
        .expect("second mixed page");
    assert!(matches!(
        &second_mixed_page.sessions[0],
        collaboration_protocol::ProviderSessionSummary::ClaudeCodeInteractive {
            updated_at: 1_790_162_494,
            ..
        }
    ));
    let invalid_source = client
        .list_provider_sessions(ProviderSessionListParams {
            source: NativeSessionSource::Interactive,
            ..params
        })
        .await
        .expect_err("Claude terminal stored view is unsupported");
    assert!(matches!(
        invalid_source,
        collaboration_client::ClientError::Rejected { code: -32050, .. }
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

    let unavailable_claude: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Claude unavailable fixture",
        "availability":{"state":"unavailable","observedAt":"2026-09-26T00:00:00Z","reason":"ACP unavailable"},
        "channels":[]
    })).expect("unavailable Claude endpoint");
    let identity = ServiceIdentity::new(service_id, epoch, &format!("sha256:{}", "a".repeat(64)))
        .expect("identity")
        .with_endpoints(vec![unavailable_claude])
        .expect("endpoint")
        .with_claude_code_sessions(Arc::new(ClaudeCodeSessionRegistry::new(
            root.path().join("claude-fixture-registry"),
        )));
    let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("third socket");
    let server = tokio::spawn(serve_control_connection(server_stream, identity));
    let mut client = ControlClient::initialize(client_stream, "terminal-only-list-test", "1")
        .await
        .expect("third initialize");
    let terminal_only = client
        .list_provider_sessions(ProviderSessionListParams {
            endpoint: target.endpoint,
            view: NativeSessionView::Active,
            scope: NativeSessionScope::Any,
            source: NativeSessionSource::Interactive,
            query: None,
            page_size: 10,
            cursor: None,
        })
        .await
        .expect("terminal discovery survives unavailable ACP");
    assert_eq!(terminal_only.sessions.len(), 1);
    assert!(matches!(
        &terminal_only.sessions[0],
        collaboration_protocol::ProviderSessionSummary::ClaudeCodeInteractive { .. }
    ));
    client.close().await.expect("third close");
    server
        .await
        .expect("third server task")
        .expect("third serve");
}
