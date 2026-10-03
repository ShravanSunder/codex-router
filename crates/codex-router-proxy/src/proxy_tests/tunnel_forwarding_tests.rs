use super::*;

#[test]
#[allow(clippy::result_large_err)]
fn blocking_websocket_tunnel_preserves_first_frame_and_sanitizes_upstream_handshake() {
    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock websocket upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock websocket upstream address should read: {error}"),
    };
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock websocket upstream should accept: {error}"),
        };
        let mut websocket = match accept_hdr(stream, |request: &Request, response: Response| {
            let authorization = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>")
                .to_owned();
            let local_token = request
                .headers()
                .get("x-codex-router-token")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if let Err(error) = upstream_sender.send((authorization, local_token)) {
                panic!("mock websocket upstream headers should record: {error}");
            }
            Ok(response)
        }) {
            Ok(websocket) => websocket,
            Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
        };
        let first_frame = match websocket.read() {
            Ok(message) => message,
            Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
        };
        if let Err(error) = upstream_sender.send((first_frame.to_string(), None)) {
            panic!("mock websocket upstream first frame should record: {error}");
        }
        if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
            panic!("mock websocket upstream should send response: {error}");
        }
    });

    let router_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("router websocket listener should bind: {error}"),
    };
    let router_address = match router_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("router websocket address should read: {error}"),
    };
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let protocol_router = WebSocketProtocolRouter::new();
    let upstream_url = format!("ws://{upstream_address}/v1/responses");
    let router_thread = thread::spawn(move || {
        let (stream, _peer_address) = match router_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("router websocket should accept local client: {error}"),
        };
        let tunnel =
            BlockingWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
        match tunnel.handle_connection(stream, upstream_url.as_str(), 1) {
            Ok(()) => {}
            Err(error) => panic!("websocket tunnel should complete: {error}"),
        }
    });

    let mut request = match format!("ws://{router_address}/v1/responses").into_client_request() {
        Ok(request) => request,
        Err(error) => panic!("local websocket request should build: {error}"),
    };
    request.headers_mut().insert(
        "X-Codex-Router-Token",
        HeaderValue::from_static("current-token"),
    );
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_static("Bearer current-token"),
    );
    let (mut client, _response) = match connect(request) {
        Ok(connection) => connection,
        Err(error) => panic!("local websocket client should connect: {error}"),
    };
    let first_frame = r#"{"type":"response.create","unknown_codex_field":{"kept":true}}"#;
    if let Err(error) = client.send(Message::text(first_frame)) {
        panic!("local websocket client should send first frame: {error}");
    }
    let response = match client.read() {
        Ok(message) => message,
        Err(error) => panic!("local websocket client should read upstream response: {error}"),
    };

    assert_eq!(response.to_string(), r#"{"type":"response.completed"}"#);
    let (authorization, local_token) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream handshake should be recorded: {error}"),
    };
    assert_eq!(authorization, "Bearer selected-upstream-token");
    assert_eq!(local_token, None);
    let (recorded_first_frame, _) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream first frame should be recorded: {error}"),
    };
    assert_eq!(recorded_first_frame, first_frame);

    match router_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("router websocket thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn async_websocket_tunnel_forwards_first_frame_and_second_local_frame() {
    let upstream_listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("mock websocket upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock websocket upstream address should read: {error}"),
    };
    let upstream_task = tokio::spawn(async move {
        let (stream, _peer_address) = match upstream_listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("mock websocket upstream should accept: {error}"),
        };
        let mut websocket = match tokio_tungstenite::accept_async(stream).await {
            Ok(websocket) => websocket,
            Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
        };
        let first_frame = match websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("mock upstream should read first frame: {error}"),
            None => panic!("mock upstream should receive first frame"),
        };
        assert_eq!(
            first_frame,
            Message::text(r#"{"type":"response.create","turn":1}"#),
        );
        if let Err(error) = websocket
            .send(Message::text(r#"{"type":"response.output_text.delta"}"#))
            .await
        {
            panic!("mock upstream should send non-terminal event: {error}");
        }
        let second_frame = match websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("mock upstream should read second frame: {error}"),
            None => panic!("mock upstream should receive second frame"),
        };
        assert_eq!(
            second_frame,
            Message::text(r#"{"type":"response.create","turn":2}"#),
        );
        if let Err(error) = websocket
            .send(Message::text(
                r#"{"type":"response.completed","response":{"id":"resp_async"}}"#,
            ))
            .await
        {
            panic!("mock upstream should send completion: {error}");
        }
        match websocket.next().await {
            Some(Ok(Message::Close(_))) => {}
            Some(Ok(message)) => panic!("mock upstream should receive close, got {message:?}"),
            Some(Err(error)) => panic!("mock upstream should read close: {error}"),
            None => panic!("mock upstream should receive close"),
        }
    });

    let local_listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("router websocket listener should bind: {error}"),
    };
    let local_address = match local_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("router websocket listener address should read: {error}"),
    };
    let auth_gate = local_auth_gate();
    let selector = RecordingAsyncSelector::default();
    let resolver = RecordingAsyncProviderCredentialResolver::new("selected-upstream-token");
    let protocol_router = WebSocketProtocolRouter::new();
    let revocations = WebSocketRevocationRegistry::new();
    let client_revocations = revocations.clone();
    let tunnel = AsyncWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
        .with_revocation_registry(revocations.clone());
    let upstream_url = format!("ws://{upstream_address}/v1/responses");
    let server_future = async {
        let (stream, _peer_address) = match local_listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("router websocket listener should accept: {error}"),
        };
        let local_websocket = match tokio_tungstenite::accept_async(stream).await {
            Ok(websocket) => websocket,
            Err(error) => panic!("local websocket should accept: {error}"),
        };
        let handshake = WebSocketHandshakeRequest::new()
            .with_header(Header::new("Authorization", "Bearer current-token"));
        tunnel
            .handle_upgraded_connection(local_websocket, handshake, &upstream_url)
            .await
    };
    let client_future = async {
        let local_url = format!("ws://{local_address}/v1/responses");
        let (mut client_websocket, _response) =
            match tokio_tungstenite::connect_async(local_url).await {
                Ok(connected) => connected,
                Err(error) => panic!("local websocket client should connect: {error}"),
            };
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(
            selector.take_recorded().is_empty(),
            "preconnect must not select before first data frame"
        );
        assert!(
            resolver.take_recorded().is_empty(),
            "preconnect must not resolve upstream credentials before first data frame"
        );
        if let Err(error) = client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
        {
            panic!("local client should send first frame: {error}");
        }
        let non_terminal = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("local client should receive delta: {error}"),
            None => panic!("local client should receive delta"),
        };
        assert_eq!(
            non_terminal,
            Message::text(r#"{"type":"response.output_text.delta"}"#),
        );
        assert_eq!(client_revocations.snapshot().active_sessions, 1);
        assert_eq!(client_revocations.snapshot().high_water_sessions, 1);
        if let Err(error) = client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await
        {
            panic!("local client should send second frame: {error}");
        }
        let completed = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("local client should receive completion: {error}"),
            None => panic!("local client should receive completion"),
        };
        assert_eq!(
            completed,
            Message::text(r#"{"type":"response.completed","response":{"id":"resp_async"}}"#),
        );
        if let Err(error) = client_websocket.close(None).await {
            panic!("local client should close after second turn: {error}");
        }
    };

    let (server_result, ()) = tokio::join!(server_future, client_future);
    match server_result {
        Ok(()) => {}
        Err(error) => panic!("async websocket tunnel should complete: {error}"),
    }
    match upstream_task.await {
        Ok(()) => {}
        Err(error) => panic!("mock upstream task should join: {error}"),
    }

    assert_eq!(
        selector.take_recorded(),
        vec![("/v1/responses".to_owned(), TokenGeneration::new(1))],
    );
    assert_eq!(resolver.take_recorded(), vec!["acct_selected".to_owned()],);
    assert_eq!(revocations.snapshot().active_sessions, 0);
    assert_eq!(revocations.snapshot().high_water_sessions, 1);
    assert_eq!(revocations.snapshot().registered_sessions, 1);
    assert_eq!(revocations.snapshot().closed_sessions, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn async_websocket_tunnel_propagates_upstream_close_to_idle_local_client() {
    let upstream_listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("mock websocket upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock websocket upstream address should read: {error}"),
    };
    let upstream_task = tokio::spawn(async move {
        let (stream, _peer_address) = match upstream_listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("mock websocket upstream should accept: {error}"),
        };
        let mut websocket = match tokio_tungstenite::accept_async(stream).await {
            Ok(websocket) => websocket,
            Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
        };
        match websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("mock upstream should read first frame: {error}"),
            None => panic!("mock upstream should receive first frame"),
        }
        if let Err(error) = websocket
            .send(Message::text(r#"{"type":"response.completed"}"#))
            .await
        {
            panic!("mock upstream should send completion: {error}");
        }
        if let Err(error) = websocket.close(None).await {
            panic!("mock upstream should close after completion: {error}");
        }
    });

    let local_listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("router websocket listener should bind: {error}"),
    };
    let local_address = match local_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("router websocket listener address should read: {error}"),
    };
    let auth_gate = local_auth_gate();
    let selector = RecordingAsyncSelector::default();
    let resolver = RecordingAsyncProviderCredentialResolver::new("selected-upstream-token");
    let protocol_router = WebSocketProtocolRouter::new();
    let tunnel = AsyncWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
    let upstream_url = format!("ws://{upstream_address}/v1/responses");
    let server_future = async {
        let (stream, _peer_address) = match local_listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("router websocket listener should accept: {error}"),
        };
        let local_websocket = match tokio_tungstenite::accept_async(stream).await {
            Ok(websocket) => websocket,
            Err(error) => panic!("local websocket should accept: {error}"),
        };
        let handshake = WebSocketHandshakeRequest::new()
            .with_header(Header::new("Authorization", "Bearer current-token"));
        tunnel
            .handle_upgraded_connection(local_websocket, handshake, &upstream_url)
            .await
    };
    let client_future = async {
        let local_url = format!("ws://{local_address}/v1/responses");
        let (mut client_websocket, _response) =
            match tokio_tungstenite::connect_async(local_url).await {
                Ok(connected) => connected,
                Err(error) => panic!("local websocket client should connect: {error}"),
            };
        if let Err(error) = client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
        {
            panic!("local client should send first frame: {error}");
        }
        let completed = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("local client should receive completion: {error}"),
            None => panic!("local client should receive completion"),
        };
        assert_eq!(completed, Message::text(r#"{"type":"response.completed"}"#));

        let close_result = tokio::time::timeout(Duration::from_secs(1), client_websocket.next())
            .await
            .unwrap_or_else(|_elapsed| panic!("local client should observe upstream close"));
        match close_result {
            Some(Ok(Message::Close(_))) | None => {}
            Some(Ok(message)) => panic!("local client should receive close, got {message:?}"),
            Some(Err(error)) => panic!("local client close should be clean: {error}"),
        }
    };

    let (server_result, ()) = tokio::join!(server_future, client_future);
    match server_result {
        Ok(()) => {}
        Err(error) => panic!("async websocket tunnel should complete: {error}"),
    }
    match upstream_task.await {
        Ok(()) => {}
        Err(error) => panic!("mock upstream task should join: {error}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn async_websocket_tunnel_handles_control_frames_before_first_data_without_selection() {
    let local_listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("router websocket listener should bind: {error}"),
    };
    let local_address = match local_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("router websocket listener address should read: {error}"),
    };
    let auth_gate = local_auth_gate();
    let selector = RecordingAsyncSelector::default();
    let resolver = RecordingAsyncProviderCredentialResolver::new("selected-upstream-token");
    let protocol_router = WebSocketProtocolRouter::new();
    let tunnel = AsyncWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
    let server_future = async {
        let (stream, _peer_address) = match local_listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("router websocket listener should accept: {error}"),
        };
        let local_websocket = match tokio_tungstenite::accept_async(stream).await {
            Ok(websocket) => websocket,
            Err(error) => panic!("local websocket should accept: {error}"),
        };
        let handshake = WebSocketHandshakeRequest::new()
            .with_header(Header::new("Authorization", "Bearer current-token"));
        tunnel
            .handle_upgraded_connection(local_websocket, handshake, "ws://127.0.0.1:1/v1/responses")
            .await
    };
    let client_future = async {
        let local_url = format!("ws://{local_address}/v1/responses");
        let (mut client_websocket, _response) =
            match tokio_tungstenite::connect_async(local_url).await {
                Ok(connected) => connected,
                Err(error) => panic!("local websocket client should connect: {error}"),
            };
        if let Err(error) = client_websocket
            .send(Message::Ping(Bytes::from_static(b"preconnect")))
            .await
        {
            panic!("local client should send preconnect ping: {error}");
        }
        let pong = tokio::time::timeout(Duration::from_secs(1), client_websocket.next())
            .await
            .unwrap_or_else(|_elapsed| panic!("local client should receive preconnect pong"));
        match pong {
            Some(Ok(Message::Pong(payload))) => {
                assert_eq!(payload, Bytes::from_static(b"preconnect"));
            }
            Some(Ok(message)) => panic!("local client should receive pong, got {message:?}"),
            Some(Err(error)) => panic!("local client pong should be valid: {error}"),
            None => panic!("local client should receive pong before close"),
        }
        assert!(
            selector.take_recorded().is_empty(),
            "control frames before data must not select an account"
        );
        assert!(
            resolver.take_recorded().is_empty(),
            "control frames before data must not resolve provider credentials"
        );
        if let Err(error) = client_websocket.close(None).await {
            panic!("local client should close cleanly before first data: {error}");
        }
    };

    let (server_result, ()) = tokio::join!(server_future, client_future);
    match server_result {
        Ok(()) => {}
        Err(error) => panic!("pre-upstream control frames should close cleanly: {error}"),
    }
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::result_large_err)]
async fn async_websocket_tunnel_sanitizes_upstream_handshake() {
    let upstream_listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("mock websocket upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock websocket upstream address should read: {error}"),
    };
    let upstream_task = tokio::spawn(async move {
        let (stream, _peer_address) = match upstream_listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("mock websocket upstream should accept: {error}"),
        };
        let mut websocket = match tokio_tungstenite::accept_hdr_async(
            stream,
            |request: &Request, response: Response| {
                let authorization = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<missing>")
                    .to_owned();
                let local_token = request
                    .headers()
                    .get("x-codex-router-token")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                assert_eq!(authorization, "Bearer selected-upstream-token");
                assert_eq!(local_token, None);
                Ok(response)
            },
        )
        .await
        {
            Ok(websocket) => websocket,
            Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
        };
        let first_frame = match websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("mock upstream should read first frame: {error}"),
            None => panic!("mock upstream should receive first frame"),
        };
        assert_eq!(
            first_frame,
            Message::text(r#"{"type":"response.create","unknown_codex_field":{"kept":true}}"#),
        );
        if let Err(error) = websocket
            .send(Message::text(r#"{"type":"response.completed"}"#))
            .await
        {
            panic!("mock upstream should send completion: {error}");
        }
    });

    let local_listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("router websocket listener should bind: {error}"),
    };
    let local_address = match local_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("router websocket listener address should read: {error}"),
    };
    let auth_gate = local_auth_gate();
    let selector = RecordingAsyncSelector::default();
    let resolver = RecordingAsyncProviderCredentialResolver::new("selected-upstream-token");
    let protocol_router = WebSocketProtocolRouter::new();
    let tunnel = AsyncWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
    let upstream_url = format!("ws://{upstream_address}/v1/responses");
    let server_future = async {
        let (stream, _peer_address) = match local_listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("router websocket listener should accept: {error}"),
        };
        let local_websocket = match tokio_tungstenite::accept_async(stream).await {
            Ok(websocket) => websocket,
            Err(error) => panic!("local websocket should accept: {error}"),
        };
        let handshake = WebSocketHandshakeRequest::new()
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_header(Header::new("Authorization", "Bearer current-token"));
        tunnel
            .handle_upgraded_connection(local_websocket, handshake, &upstream_url)
            .await
    };
    let client_future = async {
        let local_url = format!("ws://{local_address}/v1/responses");
        let (mut client_websocket, _response) =
            match tokio_tungstenite::connect_async(local_url).await {
                Ok(connected) => connected,
                Err(error) => panic!("local websocket client should connect: {error}"),
            };
        let first_frame = r#"{"type":"response.create","unknown_codex_field":{"kept":true}}"#;
        if let Err(error) = client_websocket.send(Message::text(first_frame)).await {
            panic!("local client should send first frame: {error}");
        }
        let completed = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("local client should receive completion: {error}"),
            None => panic!("local client should receive completion"),
        };
        assert_eq!(completed, Message::text(r#"{"type":"response.completed"}"#));
    };

    let (server_result, ()) = tokio::join!(server_future, client_future);
    match server_result {
        Ok(()) => {}
        Err(error) => panic!("async websocket tunnel should complete: {error}"),
    }
    match upstream_task.await {
        Ok(()) => {}
        Err(error) => panic!("mock upstream task should join: {error}"),
    }
}
