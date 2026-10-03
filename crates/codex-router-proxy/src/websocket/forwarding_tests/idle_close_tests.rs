use super::*;

#[tokio::test]
async fn reset_during_new_turn_after_prior_completion_is_clean_transport_close() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let router_upstream_websocket =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let mut upstream_websocket =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let registry = WebSocketRevocationRegistry::new();
    let session = registry.register_cancellation(TokenGeneration::new(1));
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();

    let router_task = async {
        forward_duplex_until_complete(
            router_local_websocket,
            router_upstream_websocket,
            WebSocketForwardingContext {
                session_registration: session,
                affinity_owner_recorder: None,
                async_affinity_owner_recorder: None,
                affinity_record_tasks: TaskTracker::new(),
                affinity_owner_context: None,
                provider_error_observer: None,
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown,
            },
        )
        .await
    };
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
        let first_upstream_frame = match upstream_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("first upstream frame should be valid: {error}"),
            None => panic!("upstream should receive first frame"),
        };
        assert_eq!(
            first_upstream_frame.to_string(),
            r#"{"type":"response.create","turn":1}"#
        );
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("first completion should send: {error}"));
        let first_client_response = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("first client response should be valid: {error}"),
            None => panic!("client should receive first completion"),
        };
        assert_eq!(
            first_client_response.to_string(),
            r#"{"type":"response.completed","turn":1}"#
        );

        client_websocket
            .send(Message::text(
                r#"{"type":"conversation.item.create","turn":2}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("second local frame should send: {error}"));
        let second_upstream_frame = match upstream_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("second upstream frame should be valid: {error}"),
            None => panic!("upstream should receive second frame"),
        };
        assert_eq!(
            second_upstream_frame.to_string(),
            r#"{"type":"conversation.item.create","turn":2}"#
        );
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should not invent per-turn reset semantics, got {router_result:?}"
    );
}

#[tokio::test]
async fn reset_after_idle_control_frame_remains_clean_after_completion() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let router_upstream_websocket =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let mut upstream_websocket =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let registry = WebSocketRevocationRegistry::new();
    let session = registry.register_cancellation(TokenGeneration::new(1));
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();

    let router_task = async {
        forward_duplex_until_complete(
            router_local_websocket,
            router_upstream_websocket,
            WebSocketForwardingContext {
                session_registration: session,
                affinity_owner_recorder: None,
                async_affinity_owner_recorder: None,
                affinity_record_tasks: TaskTracker::new(),
                affinity_owner_context: None,
                provider_error_observer: None,
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown,
            },
        )
        .await
    };
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("local frame should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
            None => panic!("upstream should receive first frame"),
        }
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("completion should send: {error}"));
        match client_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("client should receive completion: {error}"),
            None => panic!("client should receive completion"),
        }

        client_websocket
            .send(Message::Ping(Bytes::from_static(b"idle")))
            .await
            .unwrap_or_else(|error| panic!("idle ping should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(Message::Ping(_))) => {}
            Some(Ok(message)) => panic!("upstream should receive ping, got {message:?}"),
            Some(Err(error)) => panic!("upstream ping should be valid: {error}"),
            None => panic!("upstream should receive idle ping"),
        }
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "idle control frame after completion must not make reset look like a failed turn, got {router_result:?}"
    );
}

#[tokio::test]
async fn upstream_close_after_completion_reaches_idle_local_client() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let router_upstream_websocket =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let mut upstream_websocket =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let registry = WebSocketRevocationRegistry::new();
    let session = registry.register_cancellation(TokenGeneration::new(1));
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();

    let router_task = async {
        forward_duplex_until_complete(
            router_local_websocket,
            router_upstream_websocket,
            WebSocketForwardingContext {
                session_registration: session,
                affinity_owner_recorder: None,
                async_affinity_owner_recorder: None,
                affinity_record_tasks: TaskTracker::new(),
                affinity_owner_context: None,
                provider_error_observer: None,
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown,
            },
        )
        .await
    };
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("local frame should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
            None => panic!("upstream should receive first frame"),
        }
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("completion should send: {error}"));
        let completed = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive completion: {error}"),
            None => panic!("client should receive completion"),
        };
        assert_eq!(
            completed,
            Message::text(r#"{"type":"response.completed","turn":1}"#),
        );
        upstream_websocket
            .close(None)
            .await
            .unwrap_or_else(|error| panic!("upstream should close cleanly: {error}"));

        let close_result = tokio::time::timeout(Duration::from_secs(1), client_websocket.next())
            .await
            .unwrap_or_else(|_elapsed| panic!("local client should observe upstream close"));
        match close_result {
            Some(Ok(Message::Close(_))) | None => {}
            Some(Ok(message)) => panic!("local client should receive close, got {message:?}"),
            Some(Err(error)) => panic!("local client close should be clean: {error}"),
        }
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "upstream close should reach idle local client, got {router_result:?}"
    );
    assert_eq!(registry.snapshot().active_sessions, 0);
    assert_eq!(registry.snapshot().closed_sessions, 1);
}
