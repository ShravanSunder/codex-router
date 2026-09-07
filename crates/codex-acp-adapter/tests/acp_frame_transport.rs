use codex_acp_adapter::read_acp_frame;
use tokio::io::BufReader;

#[tokio::test]
async fn acp_frames_preserve_numeric_ids_and_reject_batches_and_truncation() {
    let bytes=b"{\"jsonrpc\":\"2.0\",\"id\":9223372036854775807,\"method\":\"initialize\",\"params\":{}}\n{\"method\":\"session/cancel\",\"params\":{}}\n";
    let mut reader = BufReader::with_capacity(7, bytes.as_slice());
    let first = read_acp_frame(&mut reader)
        .await
        .unwrap_or_else(|error| panic!("first: {error}"))
        .unwrap_or_else(|| panic!("frame"));
    assert_eq!(first["id"].as_i64(), Some(i64::MAX));
    assert!(
        read_acp_frame(&mut reader)
            .await
            .unwrap_or_else(|error| panic!("second: {error}"))
            .is_some()
    );
    assert!(
        read_acp_frame(&mut reader)
            .await
            .unwrap_or_else(|error| panic!("EOF: {error}"))
            .is_none()
    );
    for input in [b"[]\n".as_slice(), b"{\"id\":1}".as_slice()] {
        assert!(read_acp_frame(&mut BufReader::new(input)).await.is_err());
    }
}
