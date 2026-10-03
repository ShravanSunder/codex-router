use super::*;

#[tokio::test(flavor = "current_thread")]
async fn async_websocket_tunnel_records_top_level_response_owner() {
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
        if let Some(Err(error)) = websocket.next().await {
            panic!("mock upstream should read first frame: {error}");
        }
        for response in [
            r#"{"body":{"response":{"id":"resp_nested"}}}"#,
            r#"{"type":"response.created","response":{"id":"resp_ws_owner"}}"#,
            r#"{"type":"response.completed"}"#,
        ] {
            if let Err(error) = websocket.send(Message::text(response)).await {
                panic!("mock upstream should send response: {error}");
            }
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
    let recorder = RecordingAffinityOwnerRecorder::default();
    let tunnel = AsyncWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
        .with_affinity_owner_recorder(Arc::new(recorder.clone()));
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
            .with_header(Header::new("X-Codex-Router-Token", "current-token"));
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
        for expected_response in [
            r#"{"body":{"response":{"id":"resp_nested"}}}"#,
            r#"{"type":"response.created","response":{"id":"resp_ws_owner"}}"#,
            r#"{"type":"response.completed"}"#,
        ] {
            let response = match client_websocket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => panic!("local client should read response: {error}"),
                None => panic!("local client should read response"),
            };
            assert_eq!(response, Message::text(expected_response));
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

    let records = wait_for_affinity_records(&recorder, 1).await;
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.account_id().as_str(), "acct_selected");
    assert_eq!(record.credential_generation(), 1);
    assert_eq!(record.route_band(), RouteBand::Responses);
    assert_eq!(
        record.source_transport(),
        AffinitySourceTransport::WebSocket,
    );
    assert_eq!(
        record.affinity_key_hash(),
        &must_ok(hash_previous_response_id(
            &test_affinity_secret(),
            &must_ok(PreviousResponseId::new("resp_ws_owner")),
        )),
    );
    assert_ne!(record.affinity_key_hash().as_str(), "resp_ws_owner");
}

#[tokio::test(flavor = "current_thread")]
async fn async_websocket_tunnel_does_not_gate_forwarding_on_slow_affinity_recorder() {
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
        if let Some(Err(error)) = websocket.next().await {
            panic!("mock upstream should read first frame: {error}");
        }
        if let Err(error) = websocket
            .send(Message::text(
                r#"{"type":"response.completed","response":{"id":"resp_slow_recorder"}}"#,
            ))
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
    let (recorder_entered_sender, recorder_entered_receiver) = mpsc::channel();
    let (recorder_release_sender, recorder_release_receiver) = mpsc::channel();
    let recorder = Arc::new(BlockingAffinityOwnerRecorder::new(
        recorder_entered_sender,
        recorder_release_receiver,
    ));
    let affinity_record_tasks = TaskTracker::new();
    let tunnel = AsyncWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
        .with_affinity_owner_recorder(recorder.clone())
        .with_affinity_owner_task_tracker(affinity_record_tasks.clone());
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
            .with_header(Header::new("X-Codex-Router-Token", "current-token"));
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
        let completed =
            match tokio::time::timeout(Duration::from_millis(250), client_websocket.next()).await {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(error))) => panic!("local client should read completion: {error}"),
                Ok(None) => panic!("local client should read completion"),
                Err(_elapsed) => panic!("slow affinity recorder gated websocket forwarding"),
            };
        assert_eq!(
            completed,
            Message::text(
                r#"{"type":"response.completed","response":{"id":"resp_slow_recorder"}}"#
            ),
        );
    };

    let (server_result, ()) = tokio::join!(server_future, client_future);
    match server_result {
        Ok(()) => {}
        Err(error) => panic!("async websocket tunnel should complete: {error}"),
    }
    match recorder_entered_receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(()) => {}
        Err(error) => panic!("blocking recorder should start after forwarding: {error}"),
    }
    let mut drain_task = tokio::spawn({
        let affinity_record_tasks = affinity_record_tasks.clone();
        async move {
            affinity_record_tasks.close();
            affinity_record_tasks.wait().await;
        }
    });
    match tokio::time::timeout(Duration::from_millis(50), &mut drain_task).await {
        Ok(join_result) => panic!(
            "affinity recorder drain finished before blocked recorder was released: {join_result:?}"
        ),
        Err(_elapsed) => {}
    }
    if let Err(error) = recorder_release_sender.send(()) {
        panic!("blocking recorder release should send: {error}");
    }
    match tokio::time::timeout(Duration::from_secs(1), drain_task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("affinity recorder drain task should join: {error}"),
        Err(_elapsed) => panic!("affinity recorder drain should finish after release"),
    }
    for _attempt in 0..50 {
        if recorder.records_snapshot().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(recorder.records_snapshot().len(), 1);
    match upstream_task.await {
        Ok(()) => {}
        Err(error) => panic!("mock upstream task should join: {error}"),
    }
}

#[test]
#[allow(clippy::result_large_err)]
fn blocking_websocket_tunnel_records_top_level_response_owner() {
    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock websocket upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock websocket upstream address should read: {error}"),
    };
    let upstream_thread = thread::spawn(move || {
        let (stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock websocket upstream should accept: {error}"),
        };
        let mut websocket = match accept_hdr(stream, |_request: &Request, response: Response| {
            Ok(response)
        }) {
            Ok(websocket) => websocket,
            Err(error) => {
                panic!("mock websocket upstream handshake should accept: {error}")
            }
        };
        if let Err(error) = websocket.read() {
            panic!("mock websocket upstream should read first frame: {error}");
        }
        if let Err(error) = websocket.send(Message::text(
            r#"{"body":{"response":{"id":"resp_nested"}}}"#,
        )) {
            panic!("mock websocket upstream should send nested non-owner response: {error}");
        }
        if let Err(error) = websocket.send(Message::text(
            r#"{"type":"response.created","response":{"id":"resp_ws_owner"}}"#,
        )) {
            panic!("mock websocket upstream should send owner response: {error}");
        }
        if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
            panic!("mock websocket upstream should complete response: {error}");
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
    let recorder = RecordingAffinityOwnerRecorder::default();
    let recorder_for_thread = recorder.clone();
    let upstream_url = format!("ws://{upstream_address}/v1/responses");
    let router_thread = thread::spawn(move || {
        let (stream, _peer_address) = match router_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("router websocket should accept local client: {error}"),
        };
        let tunnel =
            BlockingWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
                .with_affinity_owner_recorder(&recorder_for_thread);
        match tunnel.handle_connection(stream, upstream_url.as_str(), 3) {
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
    let (mut client, _response) = match connect(request) {
        Ok(connection) => connection,
        Err(error) => panic!("local websocket client should connect: {error}"),
    };
    if let Err(error) = client.send(Message::text(r#"{"type":"response.create"}"#)) {
        panic!("local websocket client should send first frame: {error}");
    }
    for expected_response in [
        r#"{"body":{"response":{"id":"resp_nested"}}}"#,
        r#"{"type":"response.created","response":{"id":"resp_ws_owner"}}"#,
        r#"{"type":"response.completed"}"#,
    ] {
        let response = match client.read() {
            Ok(message) => message,
            Err(error) => panic!("local websocket client should read response: {error}"),
        };
        assert_eq!(response.to_string(), expected_response);
    }

    match router_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("router websocket thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }

    let records = recorder.take_records();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.account_id().as_str(), "acct_selected");
    assert_eq!(record.credential_generation(), 1);
    assert_eq!(record.route_band(), RouteBand::Responses);
    assert_eq!(
        record.source_transport(),
        AffinitySourceTransport::WebSocket
    );
    assert_eq!(
        record.affinity_key_hash(),
        &must_ok(hash_previous_response_id(
            &test_affinity_secret(),
            &must_ok(PreviousResponseId::new("resp_ws_owner")),
        ))
    );
    assert_ne!(record.affinity_key_hash().as_str(), "resp_ws_owner");
}

#[test]
#[allow(clippy::result_large_err)]
fn blocking_websocket_tunnel_pins_one_upstream_account_for_multiple_turns() {
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
            if let Err(error) = upstream_sender.send(("auth".to_owned(), authorization)) {
                panic!("mock websocket upstream auth should record: {error}");
            }
            Ok(response)
        }) {
            Ok(websocket) => websocket,
            Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
        };
        for turn in 1..=2 {
            let frame = match websocket.read() {
                Ok(message) => message,
                Err(error) => {
                    panic!("mock websocket upstream should read turn {turn}: {error}")
                }
            };
            if let Err(error) = upstream_sender.send((format!("turn-{turn}"), frame.to_string())) {
                panic!("mock websocket upstream turn should record: {error}");
            }
            if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
                panic!("mock websocket upstream should complete turn {turn}: {error}");
            }
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
    let (mut client, _response) = match connect(request) {
        Ok(connection) => connection,
        Err(error) => panic!("local websocket client should connect: {error}"),
    };
    for turn in 1..=2 {
        let frame = format!(r#"{{"type":"response.create","turn":{turn}}}"#);
        if let Err(error) = client.send(Message::text(frame)) {
            panic!("local websocket client should send turn {turn}: {error}");
        }
        let response = match client.read() {
            Ok(message) => message,
            Err(error) => panic!("local websocket client should read turn {turn}: {error}"),
        };
        assert_eq!(response.to_string(), r#"{"type":"response.completed"}"#);
    }
    if let Err(error) = client.close(None) {
        panic!("local websocket client should close: {error}");
    }

    assert_eq!(
        upstream_receiver.recv().unwrap_or_else(|error| {
            panic!("upstream auth should record: {error}");
        }),
        (
            "auth".to_owned(),
            "Bearer selected-upstream-token".to_owned()
        )
    );
    assert_eq!(
        upstream_receiver.recv().unwrap_or_else(|error| {
            panic!("upstream first turn should record: {error}");
        }),
        (
            "turn-1".to_owned(),
            r#"{"type":"response.create","turn":1}"#.to_owned()
        )
    );
    assert_eq!(
        upstream_receiver.recv().unwrap_or_else(|error| {
            panic!("upstream second turn should record: {error}");
        }),
        (
            "turn-2".to_owned(),
            r#"{"type":"response.create","turn":2}"#.to_owned()
        )
    );

    match router_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("router websocket thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }
}
