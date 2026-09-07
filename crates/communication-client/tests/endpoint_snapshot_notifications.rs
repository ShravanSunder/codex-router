//! Snapshot watermarks separate historical notifications from subsequent changes.
#[cfg(test)]
mod tests {
    use communication_client::{ClientError, ControlClient};
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
    const EPOCH: &str = "00000000-0000-4000-8000-000000000002";

    #[tokio::test]
    async fn snapshot_discards_covered_frames_buffered_on_both_sides_of_response() {
        // Arrange: sequence one precedes the response; two and three share its read buffer.
        let (mut client, peer) = fixture(vec![event(1)], vec![event(2), event(3)], None).await;
        // Act: use the snapshot as current truth, then consume only subsequent changes.
        let snapshot = client.list_endpoints().await.unwrap();
        let next = client.next_notification().await.unwrap();
        // Assert: neither historical event can regress the caller's snapshot.
        assert_eq!(snapshot.sequence, 2);
        assert_eq!(next["params"]["sequence"], 3);
        assert_eq!(next["params"]["endpoint"]["label"], "state-3");
        client.close().await.unwrap();
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_also_discards_covered_frames_arriving_after_the_call_returns() {
        // Arrange: the wire's remaining covered frame is deliberately delayed.
        let (release, wait) = tokio::sync::oneshot::channel();
        let (mut client, peer) =
            fixture(vec![event(1)], vec![event(2), event(3)], Some(wait)).await;
        assert_eq!(client.list_endpoints().await.unwrap().sequence, 2);
        // Act: make the delayed notification observable only after snapshot consumption.
        release.send(()).unwrap();
        let next = client.next_notification().await.unwrap();
        // Assert: wire arrival time does not make a covered notification new.
        assert_eq!(next["params"]["sequence"], 3);
        client.close().await.unwrap();
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_does_not_hide_a_gap_in_following_changes() {
        let (mut client, peer) = fixture(vec![], vec![event(4)], None).await;
        client.list_endpoints().await.unwrap();
        assert!(matches!(
            client.next_notification().await,
            Err(ClientError::Protocol(
                "notification scope or sequence mismatch"
            ))
        ));
        assert!(matches!(
            client.next_notification().await,
            Err(ClientError::Protocol("connection is retired"))
        ));
        client.close().await.unwrap();
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_above_snapshot_watermark_remains_a_protocol_error() {
        let (mut client, peer) = fixture(vec![], vec![event(3), event(3)], None).await;
        client.list_endpoints().await.unwrap();
        assert_eq!(
            client.next_notification().await.unwrap()["params"]["sequence"],
            3
        );
        assert!(matches!(
            client.next_notification().await,
            Err(ClientError::Protocol(
                "notification scope or sequence mismatch"
            ))
        ));
        client.close().await.unwrap();
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn covered_frame_from_another_epoch_cannot_be_silently_discarded() {
        let mut wrong_epoch = event(1);
        wrong_epoch["params"]["serviceEpoch"] = json!(SERVICE);
        let (mut client, peer) = fixture(vec![wrong_epoch], vec![], None).await;
        assert!(matches!(
            client.list_endpoints().await,
            Err(ClientError::Protocol(_))
        ));
        assert!(matches!(
            client.next_notification().await,
            Err(ClientError::Protocol("connection is retired"))
        ));
        client.close().await.unwrap();
        peer.await.unwrap();
    }

    fn description(sequence: u64) -> Value {
        json!({"endpoint":{"serviceId":SERVICE,"endpointId":"codex-local"},
            "label":format!("state-{sequence}"),"availability":{"state":"unprobed"},
            "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock",
                "schemaDigest":null,"generation":null}]})
    }

    fn event(sequence: u64) -> Value {
        json!({"jsonrpc":"2.0","method":"endpoint/changed",
            "params":{"serviceEpoch":EPOCH,"sequence":sequence,"endpoint":description(sequence)}})
    }

    async fn fixture(
        before: Vec<Value>,
        after: Vec<Value>,
        release: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> (ControlClient, tokio::task::JoinHandle<()>) {
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let peer = tokio::spawn(async move {
            let (read, mut write) = server.into_split();
            let mut lines = BufReader::new(read).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "control/initialize");
            let initialized = json!({"jsonrpc":"2.0","id":request["id"],"result":{
                "version":{"major":1,"minor":0},"serviceId":SERVICE,"serviceEpoch":EPOCH,
                "controlSchemaDigest":format!("sha256:{}","a".repeat(64))}});
            write
                .write_all(format!("{initialized}\n").as_bytes())
                .await
                .unwrap();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "endpoint/list");
            let snapshot = json!({"jsonrpc":"2.0","id":request["id"],"result":{
                "serviceEpoch":EPOCH,"sequence":2,"endpoints":[description(2)]}});
            let mut frames = before;
            frames.push(snapshot);
            if let Some(release) = release {
                write_frames(&mut write, frames).await;
                release.await.unwrap();
                write_frames(&mut write, after).await;
            } else {
                frames.extend(after);
                write_frames(&mut write, frames).await;
            }
            // Keep the peer alive until client close; teardown must not race the assertions.
            assert!(lines.next_line().await.unwrap().is_none());
        });
        (
            ControlClient::initialize(client, "snapshot-proof", "1")
                .await
                .unwrap(),
            peer,
        )
    }

    async fn write_frames(write: &mut tokio::net::unix::OwnedWriteHalf, frames: Vec<Value>) {
        let text = frames
            .into_iter()
            .map(|frame| format!("{frame}\n"))
            .collect::<String>();
        write.write_all(text.as_bytes()).await.unwrap();
    }
}
