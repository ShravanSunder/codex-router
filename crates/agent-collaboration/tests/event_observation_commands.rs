//! Executable observation proof over the real collaboration API and native Unix carriers.
#[cfg(test)]
mod tests {
    use collaboration_client::protocol::EndpointDescription;
    use collaboration_service::ServiceIdentity;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::{os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
    use tokio_tungstenite::tungstenite::Message;

    const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
    const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";

    #[derive(Clone, Copy)]
    enum AttachmentCase {
        BufferedEvents,
        NativeRejection,
        GenerationChanged,
        ByteSaturation,
        LateEvent,
        MalformedFrame,
    }

    #[tokio::test]
    async fn listener_orders_readiness_before_buffered_events_and_reports_connection_loss() {
        // Arrange / Act: the native peer emits content and a reverse request during resume.
        let output = run_observation_case(AttachmentCase::BufferedEvents, "buffered", false).await;
        let records = output_records(&output);
        // Assert: callbacks remain visible, unanswered, and in original native order.
        assert_eq!(output.status.code(), Some(3));
        assert!(output.stderr.is_empty());
        let [ready, chunk, callback, closed] = records.as_slice() else {
            panic!("expected ready, two messages and closure: {records:?}");
        };
        assert_eq!(ready["kind"], "listenerReady");
        assert_eq!(ready["target"]["sessionId"], "observed-thread");
        assert_eq!(ready["generation"]["generation"], 1);
        assert_eq!(chunk["kind"], "nativeMessage");
        assert_eq!(chunk["message"]["params"]["delta"], "buffered-output");
        assert_eq!(callback["message"]["id"], "permission-request");
        assert_eq!(
            callback["message"]["method"],
            "item/commandExecution/requestApproval"
        );
        for record in [chunk, callback] {
            assert_eq!(record["target"], ready["target"]);
            assert_eq!(record["generation"], ready["generation"]);
        }
        assert_eq!(
            closed,
            &json!({"kind":"connectionClosed","reason":"nativeConnectionLost"})
        );
    }

    #[tokio::test]
    async fn rejected_native_attachment_emits_no_listener_readiness() {
        // Arrange / Act: native resume rejects this exact identity.
        let output = run_observation_case(AttachmentCase::NativeRejection, "rejected", false).await;
        // Assert: failed attachment is never presented as an established observer.
        assert_eq!(output.status.code(), Some(5));
        let records = output_records(&output);
        let [error_record] = records.as_slice() else {
            panic!("expected one attachment failure: {records:?}");
        };
        assert_eq!(error_record["kind"], "error");
        assert_eq!(error_record["target"]["sessionId"], "observed-thread");
        assert_eq!(error_record["error"]["stage"], "observation-attach");
        assert_eq!(error_record["error"]["effect"], "unknown");
        assert_eq!(error_record["error"]["kind"], "protocolViolation");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("listenerReady"));
    }

    #[tokio::test]
    async fn replacement_during_attachment_emits_no_stale_listener_readiness() {
        // Arrange / Act: the endpoint publishes a successor before resume returns.
        let output =
            run_observation_case(AttachmentCase::GenerationChanged, "replacement", false).await;
        // Assert: buffered output from that retired generation is not advertised as ready.
        assert_eq!(output.status.code(), Some(5));
        let records = output_records(&output);
        let [error_record] = records.as_slice() else {
            panic!("expected one attachment failure: {records:?}");
        };
        assert_eq!(error_record["kind"], "error");
        assert_eq!(error_record["target"]["sessionId"], "observed-thread");
        assert_eq!(error_record["error"]["stage"], "observation-attach");
        assert_eq!(error_record["error"]["effect"], "unknown");
        assert_eq!(error_record["error"]["kind"], "protocolViolation");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("listenerReady"));
    }

    #[tokio::test]
    async fn bounded_cli_observe_returns_the_shared_sdk_result() {
        let output = run_observation_case(AttachmentCase::BufferedEvents, "bounded", true).await;
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let records = output_records(&output);
        let [result] = records.as_slice() else {
            panic!("expected one bounded result");
        };
        assert_eq!(result["target"]["sessionId"], "observed-thread");
        assert_eq!(result["generation"]["generation"], 1);
        assert_eq!(result["attached"], true);
        assert_eq!(result["endReason"], "backendDisconnected");
        assert_eq!(result["continuationGap"], true);
        assert_eq!(result["events"].as_array().map(Vec::len), Some(2));
    }

    #[tokio::test]
    async fn bounded_cli_observe_distinguishes_malformed_frame_from_clean_eof() {
        let malformed =
            run_observation_case(AttachmentCase::MalformedFrame, "malformed", true).await;
        assert_eq!(malformed.status.code(), Some(5));
        let records = output_records(&malformed);
        let [failure] = records.as_slice() else {
            panic!("expected one malformed-frame failure: {records:?}");
        };
        assert_eq!(failure["error"]["kind"], "protocolViolation");
        assert_eq!(failure["error"]["stage"], "observation-collect");
        assert_eq!(failure["error"]["effect"], "unknown");

        let clean = run_observation_case(AttachmentCase::BufferedEvents, "clean-eof", true).await;
        assert!(clean.status.success());
        let records = output_records(&clean);
        let [result] = records.as_slice() else {
            panic!("expected one clean-EOF result: {records:?}");
        };
        assert_eq!(result["endReason"], "backendDisconnected");
    }

    #[tokio::test]
    async fn bounded_cli_observe_stops_at_actual_encoded_byte_budget() {
        let output = run_observation_case(AttachmentCase::ByteSaturation, "byte-limit", true).await;
        assert!(output.status.success());
        let records = output_records(&output);
        let [result] = records.as_slice() else {
            panic!("expected one bounded result");
        };
        assert_eq!(result["attached"], true);
        assert_eq!(result["endReason"], "resultLimitReached");
        assert_eq!(result["continuationGap"], true);
        assert_eq!(result["events"].as_array().map(Vec::len), Some(0));
    }

    #[tokio::test]
    async fn independent_late_bounded_observations_remain_call_local() {
        let (first, second) = tokio::join!(
            run_observation_case(AttachmentCase::LateEvent, "concurrent-a", true),
            run_observation_case(AttachmentCase::LateEvent, "concurrent-b", true),
        );
        for output in [first, second] {
            assert!(output.status.success());
            let records = output_records(&output);
            let [result] = records.as_slice() else {
                panic!("expected one bounded result");
            };
            assert_eq!(result["attached"], true);
            assert_eq!(result["endReason"], "backendDisconnected");
            assert!(result["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|event| event["params"]["delta"] == "late-output")
            }));
        }
    }

    fn endpoint_description(generation: u64) -> EndpointDescription {
        serde_json::from_value(json!({
            "endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},
            "label":"Observation fixture",
            "availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},
            "channels":[{"kind":"nativeCodex","transport":"unixWebSocket",
                "path":"codex-native.sock","schemaDigest":null,
                "generation":{"serviceEpoch":SERVICE_EPOCH,"generation":generation}}]
        }))
        .unwrap_or_else(|error| panic!("observation fixture: {error}"))
    }

    async fn run_observation_case(
        case: AttachmentCase,
        suffix: &str,
        bounded: bool,
    ) -> std::process::Output {
        let root = PathBuf::from(format!("/tmp/event-cli-{}-{suffix}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH)
            .unwrap_or_else(|error| panic!("observation fixture: {error}"))
            .with_endpoints(vec![endpoint_description(1)])
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        let directory = identity.endpoint_directory();
        let served = collaboration_mcp::test_support::ServedCollaborationApi::start(
            &root,
            collaboration_service::CollaborationApplication::new(identity),
        )
        .await
        .unwrap_or_else(|e| panic!("serve: {e}"));
        let native = tokio::net::UnixListener::bind(root.join("codex-native.sock"))
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        let peer = tokio::spawn(async move {
            let (stream, _) = native
                .accept()
                .await
                .unwrap_or_else(|error| panic!("observation fixture: {error}"));
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .unwrap_or_else(|error| panic!("observation fixture: {error}"));
            let initialize = next_native_text(&mut socket).await;
            assert_eq!(initialize["method"], "initialize");
            send_native(
                &mut socket,
                json!({"id":initialize["id"],"result":{"userAgent":"fixture"}}),
            )
            .await;
            assert_eq!(next_native_text(&mut socket).await["method"], "initialized");
            let resume = next_native_text(&mut socket).await;
            assert_eq!(resume["method"], "thread/resume");
            assert_eq!(resume["params"], json!({"threadId":"observed-thread"}));
            if matches!(case, AttachmentCase::NativeRejection) {
                send_native(&mut socket, json!({"id":resume["id"],"error":{"code":-32602,"message":"Unknown fixture thread"}})).await;
            } else {
                if matches!(case, AttachmentCase::LateEvent) {
                    send_native(
                        &mut socket,
                        json!({"id":resume["id"],"result":{"thread":{"id":"observed-thread"}}}),
                    )
                    .await;
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    send_native(&mut socket, json!({"method":"item/agentMessage/delta","params":{"threadId":"observed-thread","turnId":"observed-turn","delta":"late-output"}})).await;
                    socket
                        .close(None)
                        .await
                        .unwrap_or_else(|error| panic!("observation fixture: {error}"));
                    return;
                }
                let delta = if matches!(case, AttachmentCase::ByteSaturation) {
                    "x".repeat(256)
                } else {
                    "buffered-output".to_owned()
                };
                send_native(&mut socket, json!({"method":"item/agentMessage/delta","params":{"threadId":"observed-thread","turnId":"observed-turn","delta":delta}})).await;
                send_native(&mut socket, json!({"id":"permission-request","method":"item/commandExecution/requestApproval","params":{"threadId":"observed-thread","turnId":"observed-turn"}})).await;
                if matches!(case, AttachmentCase::GenerationChanged) {
                    directory
                        .publish(endpoint_description(2))
                        .unwrap_or_else(|error| panic!("observation fixture: {error}"));
                }
                send_native(
                    &mut socket,
                    json!({"id":resume["id"],"result":{"thread":{"id":"observed-thread"}}}),
                )
                .await;
                if matches!(case, AttachmentCase::MalformedFrame) {
                    socket
                        .send(Message::Text("{".into()))
                        .await
                        .unwrap_or_else(|error| panic!("observation fixture: {error}"));
                    socket
                        .close(None)
                        .await
                        .unwrap_or_else(|error| panic!("observation fixture: {error}"));
                    return;
                }
            }
            if matches!(
                case,
                AttachmentCase::BufferedEvents | AttachmentCase::ByteSaturation
            ) {
                // This scenario models server loss. Rejected/stale attachment instead
                // makes the client disconnect; a server close write would race that exit.
                socket
                    .close(None)
                    .await
                    .unwrap_or_else(|error| panic!("observation fixture: {error}"));
            }
            // No turn/start, turn/interrupt or callback response is allowed from an observer.
            while let Some(Ok(frame)) = socket.next().await {
                assert!(
                    !matches!(frame, Message::Text(_)),
                    "observer emitted unsolicited native input: {frame:?}"
                );
            }
        });
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
        command.args([
            "events",
            if bounded { "observe" } else { "listen" },
            "--endpoint",
            "codex-local",
            "--session",
            "observed-thread",
            "--attach",
            "--service-directory",
        ]);
        command.arg(&root).kill_on_drop(true);
        if matches!(case, AttachmentCase::ByteSaturation) {
            command.args(["--max-events", "64", "--max-bytes", "128"]);
        }
        let output = tokio::time::timeout(Duration::from_secs(5), command.output()).await;
        served
            .stop()
            .await
            .unwrap_or_else(|e| panic!("listener: {e}"));
        let peer_result = tokio::time::timeout(Duration::from_secs(2), peer).await;
        std::fs::remove_file(root.join("codex-native.sock"))
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        std::fs::remove_dir(root).unwrap_or_else(|error| panic!("observation fixture: {error}"));
        peer_result
            .unwrap_or_else(|error| panic!("observation fixture: {error}"))
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        output
            .unwrap_or_else(|error| panic!("observation fixture: {error}"))
            .unwrap_or_else(|error| panic!("observation fixture: {error}"))
    }

    async fn next_native_text(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    ) -> Value {
        let frame = socket
            .next()
            .await
            .unwrap_or_else(|| panic!("fixture native stream ended"))
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        serde_json::from_str(
            frame
                .to_text()
                .unwrap_or_else(|error| panic!("observation fixture: {error}")),
        )
        .unwrap_or_else(|error| panic!("observation fixture: {error}"))
    }

    async fn send_native(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
        value: Value,
    ) {
        socket
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
    }

    fn output_records(output: &std::process::Output) -> Vec<Value> {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("observation fixture: {error}"))
            })
            .collect()
    }
}
