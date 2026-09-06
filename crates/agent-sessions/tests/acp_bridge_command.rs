//! Raw ACP stdio bridge preserves caller-owned negotiation and callback identifiers.
#[cfg(test)]
mod tests {
    use communication_service::{LocalControlService, ManifestPublication, ServiceIdentity};
    use serde_json::json;
    use std::{os::unix::fs::DirBuilderExt, path::PathBuf, process::Stdio, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio_util::sync::CancellationToken;

    const INITIALIZE: &str = "{\"jsonrpc\":\"2.0\",\"id\":9007199254740993,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1,\"clientCapabilities\":{}}}\n";
    const CALLBACK: &str = "{\"jsonrpc\":\"2.0\",\"id\":9007199254740995,\"method\":\"session/request_permission\",\"params\":{\"sessionId\":\"fixture\",\"options\":[]}}\n";
    const CALLBACK_REPLY: &str = "{\"jsonrpc\":\"2.0\",\"id\":9007199254740995,\"result\":{\"outcome\":{\"outcome\":\"cancelled\"}}}\n";
    const RESULT: &str = "{\"jsonrpc\":\"2.0\",\"id\":9007199254740993,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{},\"authMethods\":[]}}\n";

    #[tokio::test]
    async fn acp_stdio_bridge_forwards_exact_frames_without_initializing_or_answering_callbacks() {
        // Arrange: no Codex process; the independent ACP peer checks byte-for-byte fidelity.
        let root = PathBuf::from(format!("/tmp/acp-bridge-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let id = "00000000-0000-4000-8000-000000000001";
        let digest = format!("sha256:{}", "a".repeat(64));
        let endpoint = serde_json::from_value(json!({"endpoint":{"serviceId":id,"endpointId":"codex-local"},"label":"ACP bridge fixture","availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},"channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}",communication_protocol::ACP_SCHEMA_DIGEST)}]})).unwrap();
        let identity = ServiceIdentity::new(id, id, &digest)
            .unwrap()
            .with_endpoints(vec![endpoint])
            .unwrap();
        let listener = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
        let manifest = serde_json::from_value(json!({"version":1,"serviceId":id,"serviceEpoch":id,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest})).unwrap();
        let publication = ManifestPublication::publish(&root, &manifest).unwrap();
        let stop = CancellationToken::new();
        let service = tokio::spawn(listener.run(stop.clone()));
        let acp = tokio::net::UnixListener::bind(root.join("acp.sock")).unwrap();
        let peer = tokio::spawn(async move {
            let (stream, _) = acp.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(
                line, INITIALIZE,
                "the bridge must not initialize on the caller's behalf"
            );
            stream
                .get_mut()
                .write_all(CALLBACK.as_bytes())
                .await
                .unwrap();
            line.clear();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(
                line, CALLBACK_REPLY,
                "only the caller decides callback responses"
            );
            stream.get_mut().write_all(RESULT.as_bytes()).await.unwrap();
            stream.get_mut().shutdown().await.unwrap();
        });
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
            .args(["acp", "--endpoint", "codex-local", "--service-directory"])
            .arg(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let flow = tokio::time::timeout(Duration::from_secs(5), async {
            let mut input = child.stdin.take().ok_or("stdin missing")?;
            let mut output = BufReader::new(child.stdout.take().ok_or("stdout missing")?);
            input.write_all(INITIALIZE.as_bytes()).await?;
            let mut callback = String::new();
            output.read_line(&mut callback).await?;
            if callback != CALLBACK {
                return Err("raw ACP callback was not forwarded".into());
            }
            input.write_all(CALLBACK_REPLY.as_bytes()).await?;
            let mut result = String::new();
            output.read_line(&mut result).await?;
            if result != RESULT {
                return Err("raw ACP result was not preserved".into());
            }
            // Keep stdin open: backend EOF must still terminate the bridge process.
            let status = child.wait().await?;
            if !status.success() {
                return Err("ACP bridge exited unsuccessfully".into());
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        })
        .await;
        // Cleanup applies to both the expected initial red and the final green path.
        if child.try_wait().unwrap().is_none() {
            child.kill().await.unwrap();
        }
        let _status = child.wait().await;
        stop.cancel();
        service.await.unwrap().unwrap();
        let peer_passed = peer.is_finished();
        if !peer_passed {
            peer.abort();
        }
        let peer_result = peer.await;
        drop(publication);
        std::fs::remove_file(root.join("acp.sock")).unwrap();
        std::fs::remove_dir(root).unwrap();
        // Assert: whole executable carrier exchange completed and the peer's exact checks passed.
        flow.unwrap().unwrap();
        assert!(peer_passed);
        peer_result.unwrap();
    }
}
