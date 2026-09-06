use codex_acp_adapter::{acp_connection_channels, run_acp_transport};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn partial_input_survives_concurrent_output_and_eof_closes_router() {
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
    let (wire, mut router) = acp_connection_channels();
    let task = tokio::spawn(run_acp_transport(server, wire));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    write
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":")
        .await
        .unwrap_or_else(|error| panic!("partial: {error}"));
    router
        .output
        .send(json!({"jsonrpc":"2.0","method":"session/update","params":{}}))
        .await
        .unwrap_or_else(|error| panic!("outbound: {error}"));
    let mut output = String::new();
    read.read_line(&mut output)
        .await
        .unwrap_or_else(|error| panic!("read output: {error}"));
    let output: Value =
        serde_json::from_str(&output).unwrap_or_else(|error| panic!("output JSON: {error}"));
    assert_eq!(output["method"], "session/update");
    write
        .write_all(b"9223372036854775807,\"method\":\"initialize\",\"params\":{}}\n")
        .await
        .unwrap_or_else(|error| panic!("tail: {error}"));
    let input = router.input.recv().await.unwrap_or_else(|| panic!("input"));
    assert_eq!(input["id"].as_i64(), Some(i64::MAX));
    write
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
    task.await
        .unwrap_or_else(|error| panic!("join: {error}"))
        .unwrap_or_else(|error| panic!("transport: {error}"));
    assert!(router.closed.is_cancelled());
}
