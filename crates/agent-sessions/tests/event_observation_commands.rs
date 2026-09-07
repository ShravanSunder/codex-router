//! Executable observation proof over real Control and native Unix carriers.
#[cfg(test)]
mod tests {
    use communication_protocol::{EndpointDescription, ServiceManifest};
    use communication_service::{LocalControlService, ManifestPublication, ServiceIdentity};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::{os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_util::sync::CancellationToken;

    const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
    const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";

    #[derive(Clone, Copy)]
    enum AttachmentCase {
        BufferedEvents,
        NativeRejection,
        GenerationChanged,
    }

    #[tokio::test]
    async fn listener_orders_readiness_before_buffered_events_and_reports_connection_loss() {
        // Arrange / Act: the native peer emits content and a reverse request during resume.
        let output = run_observation_case(AttachmentCase::BufferedEvents, "buffered").await;
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
        let output = run_observation_case(AttachmentCase::NativeRejection, "rejected").await;
        // Assert: failed attachment is never presented as an established observer.
        assert_eq!(output.status.code(), Some(3));
        let records = output_records(&output);
        let [error_record] = records.as_slice() else {
            panic!("expected one attachment failure: {records:?}");
        };
        assert_eq!(error_record["kind"], "error");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("listenerReady"));
    }

    #[tokio::test]
    async fn replacement_during_attachment_emits_no_stale_listener_readiness() {
        // Arrange / Act: the endpoint publishes a successor before resume returns.
        let output = run_observation_case(AttachmentCase::GenerationChanged, "replacement").await;
        // Assert: buffered output from that retired generation is not advertised as ready.
        assert_eq!(output.status.code(), Some(3));
        let records = output_records(&output);
        let [error_record] = records.as_slice() else {
            panic!("expected one attachment failure: {records:?}");
        };
        assert_eq!(error_record["kind"], "error");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("listenerReady"));
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

    async fn run_observation_case(case: AttachmentCase, suffix: &str) -> std::process::Output {
        let root = PathBuf::from(format!("/tmp/event-cli-{}-{suffix}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        let digest = format!("sha256:{}", "a".repeat(64));
        let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH, &digest)
            .unwrap_or_else(|error| panic!("observation fixture: {error}"))
            .with_endpoints(vec![endpoint_description(1)])
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        let directory = identity.endpoint_directory();
        let control = LocalControlService::bind(&root.join("control.sock"), identity)
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        let manifest: ServiceManifest = serde_json::from_value(json!({
            "version":1,"serviceId":SERVICE_ID,"serviceEpoch":SERVICE_EPOCH,
            "control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":digest
        }))
        .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        let publication = ManifestPublication::publish(&root, &manifest)
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        let stop = CancellationToken::new();
        let service = tokio::spawn(control.run(stop.clone()));
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
                send_native(&mut socket, json!({"method":"item/agentMessage/delta","params":{"threadId":"observed-thread","turnId":"observed-turn","delta":"buffered-output"}})).await;
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
            }
            if matches!(case, AttachmentCase::BufferedEvents) {
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
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
                .args([
                    "events",
                    "listen",
                    "--endpoint",
                    "codex-local",
                    "--session",
                    "observed-thread",
                    "--attach",
                    "--service-directory",
                ])
                .arg(&root)
                .kill_on_drop(true)
                .output(),
        )
        .await;
        stop.cancel();
        service
            .await
            .unwrap_or_else(|error| panic!("observation fixture: {error}"))
            .unwrap_or_else(|error| panic!("observation fixture: {error}"));
        let peer_result = tokio::time::timeout(Duration::from_secs(2), peer).await;
        drop(publication);
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
