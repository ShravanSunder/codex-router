#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
//! The Host's real router selects provider and peer clients through Control.
#[path = "support/session_message_route_fixture.rs"]
mod route_fixture;
use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    ExternalProviderStartup,
};
use collaboration_client::ControlClient;
use collaboration_protocol::{
    DeliveryOutcome, DeliveryRejectionReason, EndpointId, EndpointRef, ProviderRequestedPolicy,
    ProviderWorkingDirectory, RouterAccess, SessionId, SessionRef,
};
use collaboration_service::{ProviderOperationStore, ProviderSessionRecord};
use route_fixture::{
    create_provider_target, message, post_thread_activity_for_sessions,
    prompt_and_approve_from_peer_provider, provider_fixture, publish_peer, send_and_wait_wake,
    wait_for_completed_provider_prompts, wait_for_prompt_text,
};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt as _;
use tokio::io::{AsyncBufReadExt as _, BufReader};

#[tokio::test]
async fn host_router_selects_peer_and_provider_without_cross_loading() {
    let root = tempfile::tempdir().expect("Host root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private Host root");
    let registry = root.path().join("peer-registry");
    std::fs::create_dir(&registry).expect("peer registry");
    let executable = root.path().join("provider.py");
    provider_fixture(&executable);
    let claude_loads = root.path().join("claude-loads.log");
    let cursor_loads = root.path().join("cursor-loads.log");
    let claude_prompts = root.path().join("claude-prompts.log");
    let cursor_prompts = root.path().join("cursor-prompts.log");
    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            directory: root.path().to_owned(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: Some(registry.clone()),
        },
        vec![
            ExternalProviderStartup::Launch(
                ExternalProviderLaunchBinding::claude(
                    executable.clone(),
                    vec![
                        "claude-held".to_owned(),
                        claude_loads.display().to_string(),
                        claude_prompts.display().to_string(),
                    ],
                )
                .expect("Claude binding"),
            ),
            ExternalProviderStartup::Launch(
                ExternalProviderLaunchBinding::cursor(
                    executable,
                    vec![
                        "cursor-held".to_owned(),
                        cursor_loads.display().to_string(),
                        cursor_prompts.display().to_string(),
                    ],
                )
                .expect("Cursor binding"),
            ),
        ],
    )
    .await
    .expect("Host startup");
    let mut client = ControlClient::connect(root.path(), "route-composition", "1")
        .await
        .expect("Control connection");
    let claude_endpoint = EndpointRef {
        service_id: runtime.service_id().clone(),
        endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("Claude endpoint"),
    };
    let cursor_endpoint = EndpointRef {
        service_id: runtime.service_id().clone(),
        endpoint_id: EndpointId::try_from("cursor-local".to_owned()).expect("Cursor endpoint"),
    };
    let actor = SessionRef {
        endpoint: EndpointRef {
            service_id: runtime.service_id().clone(),
            endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("actor endpoint"),
        },
        session_id: SessionId::try_from("route-test-actor".to_owned()).expect("actor session"),
    };
    let unloaded = SessionRef {
        endpoint: claude_endpoint.clone(),
        session_id: SessionId::try_from("recorded-unloaded".to_owned()).expect("session"),
    };
    let mut store = ProviderOperationStore::open(&root.path().join("provider-operations.sqlite"))
        .await
        .expect("provider store");
    store
        .record_session(&ProviderSessionRecord {
            target: unloaded.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("working directory"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: actor.clone(),
            approver: actor.clone(),
            updated_at_ms: 1,
        })
        .await
        .expect("record unloaded provider session");
    drop(store);

    let peer_socket = registry.join("peer.sock");
    publish_peer(&registry, &unloaded.session_id, 99, &peer_socket);
    let unsupported = client
        .send_message(message(unloaded.clone()))
        .await
        .expect("unsupported peer receipt");
    assert!(matches!(unsupported.outcome,
        DeliveryOutcome::Rejected(rejection) if rejection.reason == DeliveryRejectionReason::LiveElsewhere));
    assert!(
        !claude_loads.exists(),
        "unsupported live peer allowed provider load"
    );

    publish_peer(&registry, &unloaded.session_id, 1, &peer_socket);
    let listener = tokio::net::UnixListener::bind(&peer_socket).expect("peer socket");
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("peer accepted");
        let mut lines = BufReader::new(stream).lines();
        let _auth: Value =
            serde_json::from_str(&lines.next_line().await.expect("auth line").expect("auth"))
                .expect("auth JSON");
        let user: Value =
            serde_json::from_str(&lines.next_line().await.expect("user line").expect("user"))
                .expect("user JSON");
        user
    });
    let peer = client
        .send_message(message(unloaded))
        .await
        .expect("peer receipt");
    assert_eq!(peer.outcome, DeliveryOutcome::PeerMessageWritten);
    assert_eq!(
        peer.reachability,
        Some(collaboration_protocol::SessionReachability::ClaudeCodePeer)
    );
    assert!(
        receiver.await.expect("peer receiver")["message"]["content"]
            .as_str()
            .expect("peer content")
            .contains("routed message")
    );
    assert!(
        !claude_loads.exists(),
        "writable peer allowed provider load"
    );

    std::fs::remove_file(registry.join(format!("{}.json", std::process::id())))
        .expect("remove peer registry record");
    let loaded = client
        .send_message(message(SessionRef {
            endpoint: claude_endpoint.clone(),
            session_id: SessionId::try_from("recorded-unloaded".to_owned()).expect("session"),
        }))
        .await
        .expect("provider load receipt");
    assert_eq!(loaded.outcome, DeliveryOutcome::Started);
    assert_eq!(
        loaded.reachability,
        Some(collaboration_protocol::SessionReachability::ProviderAcp)
    );
    assert_eq!(
        std::fs::read_to_string(&claude_loads).expect("load marker"),
        "load\n"
    );

    let held_claude =
        create_provider_target(&mut client, claude_endpoint, actor.clone(), root.path()).await;
    let held_cursor =
        create_provider_target(&mut client, cursor_endpoint, actor, root.path()).await;
    publish_peer(
        &registry,
        &held_claude.session_id,
        1,
        &registry.join("unused-peer.sock"),
    );
    let claude = client
        .send_message(message(held_claude.clone()))
        .await
        .expect("held Claude receipt");
    assert_eq!(claude.outcome, DeliveryOutcome::Started);
    assert_eq!(
        claude.reachability,
        Some(collaboration_protocol::SessionReachability::ProviderAcp)
    );
    let cursor = client
        .send_message(message(held_cursor.clone()))
        .await
        .expect("held Cursor receipt");
    assert_eq!(cursor.outcome, DeliveryOutcome::Started);
    assert_eq!(
        cursor.reachability,
        Some(collaboration_protocol::SessionReachability::ProviderAcp)
    );
    assert_eq!(
        std::fs::read_to_string(&claude_loads).expect("load marker"),
        "load\n",
        "held Claude was loaded again"
    );
    assert!(!cursor_loads.exists(), "held Cursor was loaded again");

    send_and_wait_wake(&mut client, held_claude.clone()).await;
    send_and_wait_wake(&mut client, held_cursor.clone()).await;
    prompt_and_approve_from_peer_provider(
        &mut client,
        held_claude.clone(),
        held_cursor.clone(),
        &cursor_prompts,
    )
    .await;
    prompt_and_approve_from_peer_provider(
        &mut client,
        held_cursor.clone(),
        held_claude.clone(),
        &claude_prompts,
    )
    .await;
    std::fs::write(&claude_prompts, "").expect("clear Claude prompt log before listen proof");
    std::fs::write(&cursor_prompts, "").expect("clear Cursor prompt log before listen proof");
    post_thread_activity_for_sessions(&mut client, [held_claude, held_cursor]).await;
    wait_for_prompt_text(&claude_prompts, "Thread activity").await;
    wait_for_prompt_text(&cursor_prompts, "Thread activity").await;
    wait_for_prompt_text(&claude_prompts, "batchesDelivered").await;
    wait_for_prompt_text(&cursor_prompts, "batchesDelivered").await;
    wait_for_completed_provider_prompts(&claude_prompts, 2).await;
    wait_for_completed_provider_prompts(&cursor_prompts, 2).await;

    runtime.shutdown().await.expect("Host shutdown");
}
