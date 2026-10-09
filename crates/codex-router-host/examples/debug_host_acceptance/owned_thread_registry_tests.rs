use super::*;
use futures_util::{SinkExt, StreamExt};

#[tokio::test]
async fn proof_creation_selects_luna_and_rejects_model_substitution() {
    // Arrange: a real local carrier whose backend returns a different model.
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let wire = tokio_tungstenite::WebSocketStream::from_raw_socket(
        client,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    let backend = tokio::spawn(async move {
        let mut server = tokio_tungstenite::WebSocketStream::from_raw_socket(
            server,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        let request: Value =
            serde_json::from_str(server.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        server
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"id":request["id"],"result":{
                    "thread":{"id":"wrong-model-thread"},
                    "model":"gpt-5.6-sol","modelProvider":"codex-router-debug",
                    "sandbox":{"type":"readOnly"},"approvalPolicy":"never"
                }})
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        request
    });
    let mut client = NativeProtocolConnection::from_websocket(wire);
    let mut registry = OwnedThreadRegistry::default();
    // Act: no real model is invoked by this protocol fixture.
    let result = registry.create(&mut client, Path::new("/tmp")).await;
    let request = backend.await.unwrap();
    // Assert: explicit model selection and fail-closed ownership admission.
    assert_eq!(
        request.pointer("/params/model").and_then(Value::as_str),
        Some("gpt-6-luna")
    );
    assert_eq!(
        request.pointer("/params/config/model_reasoning_effort"),
        Some(&json!("medium"))
    );
    assert_eq!(
        request
            .pointer("/params/approvalsReviewer")
            .and_then(Value::as_str),
        Some("user")
    );
    assert_eq!(
        request.pointer("/params/config/features.hooks"),
        Some(&json!(false))
    );
    assert!(result.is_err());
    assert!(registry.require_owned("wrong-model-thread").is_err());
}
#[test]
fn discovery_or_caller_text_does_not_grant_test_ownership() {
    // Arrange: one explicit creation receipt, unrelated user-provided IDs.
    let registry = OwnedThreadRegistry {
        threads: BTreeSet::from(["created-here".to_owned()]),
    };
    // Act / Assert.
    assert!(registry.require_owned("created-here").is_ok());
    assert!(registry.require_owned("existing-user-thread").is_err());
    assert!(registry.require_owned("").is_err());
}

#[tokio::test]
async fn owned_fork_preserves_source_contract_and_rejects_wrong_child_policy() {
    for (sandbox, effort, succeeds) in [
        ("readOnly", Some("medium"), true),
        ("dangerFullAccess", Some("medium"), false),
        ("readOnly", Some("low"), false),
        ("readOnly", None, false),
    ] {
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let wire = tokio_tungstenite::WebSocketStream::from_raw_socket(
            client,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let backend = tokio::spawn(async move {
            let mut server = tokio_tungstenite::WebSocketStream::from_raw_socket(
                server,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            let request: Value =
                serde_json::from_str(server.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(request["method"], "thread/fork");
            assert_eq!(
                request["params"],
                json!({"threadId":"owned-parent","cwd":"/source/fixture","deferGoalContinuation":true})
            );
            server.send(tokio_tungstenite::tungstenite::Message::Text(json!({"id":request["id"],"result":{"thread":{"id":"owned-child"},"model":PROOF_MODEL,"reasoningEffort":effort,"modelProvider":"codex-router-debug","cwd":"/source/fixture","sandbox":{"type":sandbox},"approvalPolicy":"never","approvalsReviewer":"user"}}).to_string().into())).await.unwrap();
        });
        let mut client = NativeProtocolConnection::from_websocket(wire);
        let mut registry = OwnedThreadRegistry {
            threads: BTreeSet::from(["owned-parent".to_owned()]),
        };
        let result = registry
            .fork_owned_thread(&mut client, "owned-parent", Path::new("/source/fixture"))
            .await;
        assert_eq!(result.is_ok(), succeeds);
        assert!(registry.require_owned("owned-parent").is_ok());
        assert!(
            registry.require_owned("owned-child").is_ok(),
            "a created child remains identified even if policy validation fails"
        );
        backend.await.unwrap();
    }
}

#[tokio::test]
async fn unowned_fork_cannot_write_a_native_frame() {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let wire = tokio_tungstenite::WebSocketStream::from_raw_socket(
        client,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    let mut client = NativeProtocolConnection::from_websocket(wire);
    let mut registry = OwnedThreadRegistry::default();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        registry.fork_owned_thread(&mut client, "existing-user-thread", Path::new("/tmp")),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    let mut bytes = [0_u8; 128];
    assert!(
        matches!(server.try_read(&mut bytes), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[tokio::test]
async fn terminal_error_for_the_owned_turn_does_not_wait_for_a_marker() {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let wire = tokio_tungstenite::WebSocketStream::from_raw_socket(
        client,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    let backend = tokio::spawn(async move {
        let mut server = tokio_tungstenite::WebSocketStream::from_raw_socket(
            server,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        server.send(tokio_tungstenite::tungstenite::Message::Text(json!({"method":"error","params":{"threadId":"owned-parent","turnId":"owned-turn","willRetry":false,"error":{"codexErrorInfo":"responseStreamDisconnected","message":"private backend cause"}}}).to_string().into())).await.unwrap();
        std::future::pending::<()>().await;
    });
    let mut client = NativeProtocolConnection::from_websocket(wire);
    let registry = OwnedThreadRegistry {
        threads: BTreeSet::from(["owned-parent".to_owned()]),
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        registry.observe_turn(&mut client, "owned-parent", "owned-turn"),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    backend.abort();
    let _joined = backend.await;
}

#[tokio::test]
async fn unowned_submission_cannot_write_a_native_frame() {
    // Arrange: a real carrier with no server protocol implementation.
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let wire = tokio_tungstenite::WebSocketStream::from_raw_socket(
        client,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    let mut client = NativeProtocolConnection::from_websocket(wire);
    let registry = OwnedThreadRegistry::default();
    // Act: ownership rejection must complete before a request/response exchange can begin.
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        registry.submit_text(&mut client, "existing-user-thread", "must not send"),
    )
    .await
    .unwrap();
    // Assert: no frame was submitted and the untouched carrier is still open.
    assert!(result.is_err());
    let mut bytes = [0_u8; 128];
    assert!(
        matches!(server.try_read(&mut bytes), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[tokio::test]
async fn owned_creation_rejects_missing_or_substituted_effort_before_any_turn() {
    for (effort, succeeds) in [(Some("medium"), true), (Some("low"), false), (None, false)] {
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let wire = tokio_tungstenite::WebSocketStream::from_raw_socket(
            client,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let backend = tokio::spawn(async move {
            let mut server = tokio_tungstenite::WebSocketStream::from_raw_socket(
                server,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            let request: Value =
                serde_json::from_str(server.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(request["method"], "thread/start");
            server.send(tokio_tungstenite::tungstenite::Message::Text(json!({"id":request["id"], "result":{
                "thread":{"id":"proof-child"}, "model":"gpt-6-luna", "reasoningEffort":effort,
                "modelProvider":"codex-router-debug", "sandbox":{"type":"readOnly"},
                "approvalPolicy":"never", "approvalsReviewer":"user"
            }}).to_string().into())).await.unwrap();
        });
        let mut client = NativeProtocolConnection::from_websocket(wire);
        let mut registry = OwnedThreadRegistry::default();
        let result = registry.create(&mut client, Path::new("/tmp")).await;
        assert_eq!(result.is_ok(), succeeds);
        assert_eq!(registry.require_owned("proof-child").is_ok(), succeeds);
        backend.await.unwrap();
    }
}

#[tokio::test]
async fn owned_turn_serializes_current_model_and_explicit_effort() {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let wire = tokio_tungstenite::WebSocketStream::from_raw_socket(
        client,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    let backend = tokio::spawn(async move {
        let mut server = tokio_tungstenite::WebSocketStream::from_raw_socket(
            server,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        let request: Value =
            serde_json::from_str(server.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(request["method"], "turn/start");
        assert_eq!(
            request["params"],
            json!({"threadId":"owned-parent", "model":"gpt-6-luna", "effort":"medium", "input":[{"type":"text", "text":"marker"}]})
        );
        server
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"id":request["id"], "result":{"turn":{"id":"owned-turn"}}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
    });
    let mut client = NativeProtocolConnection::from_websocket(wire);
    let registry = OwnedThreadRegistry {
        threads: BTreeSet::from(["owned-parent".to_owned()]),
    };
    let receipt = registry
        .submit_text(&mut client, "owned-parent", "marker")
        .await
        .unwrap();
    assert_eq!(receipt.thread_id, "owned-parent");
    assert_eq!(receipt.turn_id, "owned-turn");
    backend.await.unwrap();
}
