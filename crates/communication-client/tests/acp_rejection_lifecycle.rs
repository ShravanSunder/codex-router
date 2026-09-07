//! Failed ACP setup cannot authorize a later prompt or erase protocol error data.
#[cfg(test)]
mod tests {
    use communication_client::{AcpConversation, ClientError, ConversationEvent};
    use communication_service::{LocalControlService, ManifestPublication, ServiceIdentity};
    use serde_json::{Value, json};
    use std::{os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn rejected_load_does_not_leave_a_promptable_target_and_preserves_error_data() {
        // Arrange / Act: a valid ACP rejection contains structured policy context.
        let result = exercise_rejected_load(
            "valid-error",
            json!({
                "code":-32602,"message":"Fixture load rejected","data":{"reason":"fixture-policy"}
            }),
            false,
        )
        .await;
        // Assert: neither a rejected target nor the prior target is silently selected.
        assert!(
            matches!(result.load_error, ClientError::Rejected {code:-32602, data:Some(ref data)} if data == &json!({"reason":"fixture-policy"}))
        );
        assert!(
            result.next_failed,
            "a rejected load must require successful setup before prompting"
        );
        assert!(
            result.extra_request.is_none(),
            "unexpected native work: {:?}",
            result.extra_request
        );
        assert_eq!(result.ready_count, 0);
    }

    #[tokio::test]
    async fn malformed_acp_error_keeps_connection_unusable() {
        // Arrange / Act: Error requires both code and message in the pinned ACP schema.
        let result = exercise_rejected_load("malformed-error", json!({"code":-32602}), true).await;
        // Assert: malformed framing is not a settled, reusable protocol rejection.
        assert!(matches!(result.load_error, ClientError::Protocol(_)));
        assert!(result.next_failed);
        assert!(result.extra_request.is_none());
        assert_eq!(result.ready_count, 0);
    }

    struct RejectionResult {
        load_error: ClientError,
        next_failed: bool,
        extra_request: Option<Value>,
        ready_count: usize,
    }

    async fn exercise_rejected_load(suffix: &str, error: Value, reopen: bool) -> RejectionResult {
        let root = PathBuf::from(format!(
            "/tmp/acp-rejection-{}-{suffix}",
            std::process::id()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap_or_else(|error| panic!("fixture directory: {error}"));
        let service_id = "00000000-0000-4000-8000-000000000001";
        let digest = format!("sha256:{}", "a".repeat(64));
        let endpoint = serde_json::from_value(json!({
            "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
            "label":"ACP rejection fixture",
            "availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},
            "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock",
                "schemaDigest":format!("sha256:{}",communication_protocol::ACP_SCHEMA_DIGEST)}]
        }))
        .unwrap_or_else(|error| panic!("fixture endpoint: {error}"));
        let identity = ServiceIdentity::new(service_id, service_id, &digest)
            .and_then(|identity| identity.with_endpoints(vec![endpoint]))
            .unwrap_or_else(|error| panic!("fixture identity: {error}"));
        let listener = LocalControlService::bind(&root.join("control.sock"), identity)
            .unwrap_or_else(|error| panic!("fixture listener: {error}"));
        let manifest = serde_json::from_value(json!({"version":1,"serviceId":service_id,
        "serviceEpoch":service_id,"control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest}))
        .unwrap_or_else(|error| panic!("fixture manifest: {error}"));
        let publication = ManifestPublication::publish(&root, &manifest)
            .unwrap_or_else(|error| panic!("fixture publication: {error}"));
        let stop = CancellationToken::new();
        let service = tokio::spawn(listener.run(stop.clone()));
        let listener = tokio::net::UnixListener::bind(root.join("acp.sock"))
            .unwrap_or_else(|error| panic!("ACP socket: {error}"));
        let peer = tokio::spawn(async move {
            let (stream, _) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("accept: {error}"));
            let mut stream = BufReader::new(stream);
            let initialize = read_request(&mut stream).await;
            assert_eq!(initialize["method"], "initialize");
            write_response(
                &mut stream,
                json!({"jsonrpc":"2.0","id":initialize["id"],"result":{
            "protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}}),
            )
            .await;
            let load = read_request(&mut stream).await;
            assert_eq!(load["method"], "session/load");
            assert_eq!(load["params"]["sessionId"], "rejected-thread");
            write_response(
                &mut stream,
                json!({"jsonrpc":"2.0","id":load["id"],"error":error}),
            )
            .await;
            let mut line = String::new();
            if stream
                .read_line(&mut line)
                .await
                .unwrap_or_else(|error| panic!("post-rejection read: {error}"))
                == 0
            {
                return None;
            }
            let extra: Value = serde_json::from_str(&line)
                .unwrap_or_else(|error| panic!("extra request: {error}"));
            let result = if extra["method"] == "session/prompt" {
                json!({"stopReason":"end_turn"})
            } else {
                json!({})
            };
            write_response(
                &mut stream,
                json!({"jsonrpc":"2.0","id":extra["id"],"result":result}),
            )
            .await;
            Some(extra)
        });
        let mut client = AcpConversation::connect(
            &root,
            "codex-local"
                .to_owned()
                .try_into()
                .unwrap_or_else(|error| panic!("endpoint: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"));
        let mut ready_count = 0;
        let mut emit = |event| {
            if matches!(event, ConversationEvent::SessionReady(_)) {
                ready_count += 1;
            }
            Ok(())
        };
        let load_error = client
            .open_session(Some("rejected-thread"), &root, &mut emit)
            .await
            .err()
            .unwrap_or_else(|| panic!("fixture rejection must fail setup"));
        let next_failed = if reopen {
            client
                .open_session(Some("another-thread"), &root, &mut emit)
                .await
                .is_err()
        } else {
            client
                .prompt(
                    "must not reach the peer",
                    Duration::from_secs(2),
                    CancellationToken::new(),
                    &mut emit,
                )
                .await
                .is_err()
        };
        drop(client);
        let extra_request = tokio::time::timeout(Duration::from_secs(3), peer)
            .await
            .unwrap_or_else(|error| panic!("peer deadline: {error}"))
            .unwrap_or_else(|error| panic!("peer join: {error}"));
        stop.cancel();
        service
            .await
            .unwrap_or_else(|error| panic!("service join: {error}"))
            .unwrap_or_else(|error| panic!("service: {error}"));
        drop(publication);
        std::fs::remove_file(root.join("acp.sock"))
            .unwrap_or_else(|error| panic!("socket cleanup: {error}"));
        std::fs::remove_dir(root).unwrap_or_else(|error| panic!("directory cleanup: {error}"));
        RejectionResult {
            load_error,
            next_failed,
            extra_request,
            ready_count,
        }
    }

    async fn read_request(stream: &mut BufReader<tokio::net::UnixStream>) -> Value {
        let mut line = String::new();
        stream
            .read_line(&mut line)
            .await
            .unwrap_or_else(|error| panic!("request read: {error}"));
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("request JSON: {error}"))
    }
    async fn write_response(stream: &mut BufReader<tokio::net::UnixStream>, response: Value) {
        stream
            .get_mut()
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap_or_else(|error| panic!("response write: {error}"));
    }
}
