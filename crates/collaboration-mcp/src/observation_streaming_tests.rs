//! A bounded observation streams each event to its caller while the call is still open, on
//! both listeners, then ends at its bound with every event in its result and the cursor a
//! later call resumes from without losing an event.
use crate::COLLABORATION_API_PATH;
use crate::api_test_harness::{
    ServedApi, TEST_PROTOCOL_VERSION, TEST_SERVICE_ID, api_config, test_identity,
};
use collaboration_protocol::{
    EndpointDescription, OBSERVATION_EVENT_NOTIFICATION, ProviderRequestedPolicy,
    ProviderWorkingDirectory, RouterAccess, SessionRef,
};
use collaboration_service::{
    CollaborationApplication, ProviderOperationStore, ProviderSessionEventHub,
    ProviderSessionRecord, ServiceIdentity,
};
use rmcp::{
    ClientHandler, RoleClient,
    model::{CallToolRequestParams, CustomNotification},
    service::NotificationContext,
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};

/// An MCP client that hands the test each observation event notification it receives.
struct ObservationEvents(mpsc::UnboundedSender<Value>);

impl ClientHandler for ObservationEvents {
    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) {
        if notification.method == OBSERVATION_EVENT_NOTIFICATION {
            let _received = self.0.send(notification.params.unwrap_or(Value::Null));
        }
    }
}

fn private_directory() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt as _;
    // Unix socket paths are short; the system temporary directory can exceed the limit.
    let directory = tempfile::tempdir_in("/tmp").expect("private test directory");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private directory permissions");
    directory
}

fn provider_target(session_id: &str) -> SessionRef {
    serde_json::from_value(json!({
        "endpoint": {"serviceId": TEST_SERVICE_ID, "endpointId": "claude-local"},
        "sessionId": session_id
    }))
    .expect("provider target")
}

/// A Router with one provider endpoint whose Session hub records `sessions`.
async fn provider_router(
    directory: &Path,
    sessions: &[&SessionRef],
) -> (ServiceIdentity, Arc<ProviderSessionEventHub>) {
    let owner: SessionRef = serde_json::from_value(json!({
        "endpoint": {"serviceId": TEST_SERVICE_ID, "endpointId": "codex-local"},
        "sessionId": "owner"
    }))
    .expect("owner");
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&directory.join("operations.sqlite"))
            .await
            .expect("provider operation store"),
    ));
    for &session in sessions {
        store
            .lock()
            .await
            .record_session(&ProviderSessionRecord {
                target: session.clone(),
                working_directory: ProviderWorkingDirectory::try_from(
                    directory.display().to_string(),
                )
                .expect("working directory"),
                requested_policy: ProviderRequestedPolicy {
                    access: RouterAccess::WriteRestricted,
                },
                created_by: owner.clone().into(),
                approver: owner.clone().into(),
                updated_at_ms: 1,
            })
            .await
            .expect("session record");
    }
    let hub = Arc::new(ProviderSessionEventHub::with_capacity(store, 16));
    let identity = test_identity().with_provider_session_hub(hub.clone());
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint": {"serviceId": TEST_SERVICE_ID, "endpointId": "claude-local"},
        "label": "Claude fixture",
        "availability": {"state": "available", "observedAt": "2026-09-26T00:00:00Z"},
        "channels": [{"kind": "externalProvider", "transport": "stdioAcp",
            "bindingId": "fixture-binding", "bindingGeneration": 7,
            "runtime": {"provider": "claudeCode", "runtimeName": "fixture"},
            "capabilities": [{"name": "create", "status": "supported", "evidence": "advertised"}]}]
    }))
    .expect("provider endpoint");
    identity
        .endpoint_directory()
        .publish(description)
        .expect("endpoint publication");
    (identity, hub)
}

async fn publish(hub: &ProviderSessionEventHub, target: &SessionRef, item_id: &str) {
    publish_text(hub, target, item_id, item_id).await;
}

async fn publish_text(
    hub: &ProviderSessionEventHub,
    target: &SessionRef,
    item_id: &str,
    text: &str,
) {
    let session = serde_json::from_value(serde_json::to_value(target).expect("target JSON"))
        .expect("hub target");
    let event = serde_json::from_value(json!({
        "kind": "itemStarted",
        "item": {"itemId": item_id, "kind": {"kind": "agentMessage"}, "text": text}
    }))
    .expect("session event");
    hub.publish(session, event).await.expect("hub event");
}

/// One raw tool call over the Unix socket, as written by an HTTP client.
fn raw_tool_call(name: &str, arguments: &Value, connection: &str) -> Vec<u8> {
    let body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    }))
    .expect("tool call JSON");
    let mut request = format!(
        "POST {COLLABORATION_API_PATH} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {TEST_PROTOCOL_VERSION}\r\nConnection: {connection}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(&body);
    request
}

async fn next_streamed(streamed: &mut mpsc::UnboundedReceiver<Value>) -> Value {
    tokio::time::timeout(Duration::from_secs(5), streamed.recv())
        .await
        .expect("an event streamed while the call was open")
        .expect("notification channel open")
}

fn observe(
    target: &SessionRef,
    timeout_seconds: u64,
    cursor: Option<&Value>,
) -> CallToolRequestParams {
    let mut arguments = json!({
        "target": target, "timeoutSeconds": timeout_seconds,
        "maxEvents": 2, "maxBytes": 262_144
    });
    if let Some(cursor) = cursor {
        arguments["afterSequence"] = cursor["afterSequence"].clone();
        arguments["epoch"] = cursor["epoch"].clone();
    }
    CallToolRequestParams::new("events_observe").with_arguments(
        arguments
            .as_object()
            .expect("observation arguments")
            .clone(),
    )
}

#[tokio::test]
async fn a_bounded_observation_streams_each_event_while_the_call_is_open_on_both_listeners() {
    // Arrange: one provider Session per listener, each with one event already retained.
    let directory = private_directory();
    let over_tcp = provider_target("observed-over-tcp");
    let over_unix = provider_target("observed-over-unix");
    let (identity, hub) = provider_router(directory.path(), &[&over_tcp, &over_unix]).await;
    let config = api_config(CollaborationApplication::new(identity), directory.path());
    let tcp = ServedApi::tcp(&config).await;
    let unix = ServedApi::unix(&config, &directory.path().join("api.sock")).await;

    for (api, target) in [(&tcp, &over_tcp), (&unix, &over_unix)] {
        publish(&hub, target, "retained").await;
        let (events, mut streamed) = mpsc::unbounded_channel();
        let client = api.client_with(ObservationEvents(events)).await;
        let peer = client.peer().clone();

        // Act: a call bounded at two events and thirty seconds.
        let first_call = {
            let peer = peer.clone();
            let request = observe(target, 30, None);
            tokio::spawn(async move { peer.call_tool(request).await })
        };
        let retained = next_streamed(&mut streamed).await;
        let still_open = !first_call.is_finished();
        publish(&hub, target, "live").await;
        let first = first_call
            .await
            .expect("first call joins")
            .expect("first call answers");
        let live = next_streamed(&mut streamed).await;
        // Published between the calls; the second call resumes from the first's cursor.
        publish(&hub, target, "between-calls").await;
        let second = peer
            .call_tool(observe(target, 1, Some(&live["cursor"])))
            .await
            .expect("second call answers");
        let resumed = next_streamed(&mut streamed).await;

        // Assert: the retained event arrived while the call was open, with its cursor.
        let epoch = first
            .structured_content
            .as_ref()
            .map(|result| result["epoch"].clone())
            .expect("first result");
        assert!(
            still_open,
            "the call ended before its first event was streamed"
        );
        assert_eq!(retained["event"]["sequence"], 1);
        assert_eq!(
            retained["cursor"],
            json!({"epoch": epoch, "afterSequence": 1})
        );
        assert_eq!(live["cursor"], json!({"epoch": epoch, "afterSequence": 2}));
        // The first call ended at its event bound; its result still lists both events.
        let first = first.structured_content.expect("first result");
        assert_eq!(first["endReason"], "resultLimitReached");
        assert_eq!(first["events"], json!([retained["event"], live["event"]]));
        // The second call lost nothing between calls and ended at its deadline.
        let second = second.structured_content.expect("second result");
        assert_eq!(resumed["event"]["sequence"], 3);
        assert_eq!(second["events"], json!([resumed["event"]]));
        assert_eq!(second["endReason"], "deadlineReached");
        client.cancel().await.expect("close the client");
    }
    tcp.stop().await;
    unix.stop().await;
}

#[tokio::test]
async fn a_reader_that_stops_reading_neither_keeps_the_call_past_its_bound_nor_holds_its_slot() {
    // Arrange: one request slot on the Unix listener, and far more retained event bytes than
    // the notification channel, the HTTP writer and the socket buffers can hold.
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let directory = private_directory();
    let target = provider_target("observed-by-a-stalled-reader");
    let (identity, hub) = provider_router(directory.path(), &[&target]).await;
    for index in 0..130 {
        publish_text(&hub, &target, &format!("item-{index}"), &"x".repeat(7_000)).await;
    }
    let mut config = api_config(CollaborationApplication::new(identity), directory.path());
    config.concurrent_requests = 1;
    let socket = directory.path().join("api.sock");
    let api = ServedApi::unix(&config, &socket).await;
    let observation = json!({
        "target": target, "timeoutSeconds": 1, "maxEvents": 4096, "maxBytes": 1_048_576
    });

    // Act: the reader sends the call and then reads nothing.
    let mut stalled_reader = tokio::net::UnixStream::connect(&socket)
        .await
        .expect("connect the stalled reader");
    stalled_reader
        .write_all(&raw_tool_call("events_observe", &observation, "keep-alive"))
        .await
        .expect("send the observation");
    api.await_active_calls(1, Duration::from_secs(5)).await;

    // Assert: the call ends at its bound although the reader still has not read.
    api.await_active_calls(0, Duration::from_secs(5)).await;
    // Assert: the listener's only slot is free for the next caller meanwhile.
    let mut next_caller = tokio::net::UnixStream::connect(&socket)
        .await
        .expect("connect the next caller");
    next_caller
        .write_all(&raw_tool_call("endpoints_list", &json!({}), "close"))
        .await
        .expect("send the next call");
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), next_caller.read_to_end(&mut answer))
        .await
        .expect("the next call is answered")
        .expect("read the next answer");
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.contains("\"endpoints\""), "{answer}");
    assert!(
        !answer.contains("overloaded"),
        "the slot was still held: {answer}"
    );
    drop(stalled_reader);
    api.stop().await;
}
