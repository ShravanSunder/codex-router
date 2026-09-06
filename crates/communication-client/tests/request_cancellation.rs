use communication_client::{ClientError, ControlClient};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn dropping_inflight_call_retires_connection_without_resubmission() {
    let (client, server) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let (submitted, observed) = tokio::sync::oneshot::channel();
    let (finish, finished) = tokio::sync::oneshot::channel();
    let fixture = tokio::spawn(async move {
        let (reader, mut writer) = server.into_split();
        let mut lines = BufReader::new(reader).lines();
        let request: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .unwrap_or_else(|e| panic!("read: {e}"))
                .unwrap_or_else(|| panic!("initialize")),
        )
        .unwrap_or_else(|e| panic!("JSON: {e}"));
        let response = json!({"jsonrpc":"2.0","id":request["id"],"result":{"version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":"00000000-0000-4000-8000-000000000002","controlSchemaDigest":format!("sha256:{}","a".repeat(64))}});
        writer
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap_or_else(|e| panic!("write: {e}"));
        assert!(
            lines
                .next_line()
                .await
                .unwrap_or_else(|e| panic!("read: {e}"))
                .is_some()
        );
        submitted.send(()).unwrap_or_else(|_| panic!("signal"));
        finished.await.unwrap_or_else(|e| panic!("finish: {e}"));
        assert!(
            lines
                .next_line()
                .await
                .unwrap_or_else(|e| panic!("EOF: {e}"))
                .is_none()
        );
    });
    let mut client = ControlClient::initialize(client, "test", "1")
        .await
        .unwrap_or_else(|e| panic!("initialize: {e}"));
    {
        let request = client.list_endpoints();
        tokio::pin!(request);
        tokio::select! {
            _=observed=>{},
            result=&mut request=>panic!("request should still be pending: {result:?}"),
        }
    }
    let result = tokio::time::timeout(Duration::from_millis(100), client.list_endpoints()).await;
    assert!(matches!(
        result,
        Ok(Err(ClientError::Protocol("connection is retired")))
    ));
    drop(client);
    finish.send(()).unwrap_or_else(|_| panic!("finish signal"));
    fixture.await.unwrap_or_else(|e| panic!("join: {e}"));
}
