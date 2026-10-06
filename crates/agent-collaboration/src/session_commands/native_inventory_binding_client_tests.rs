//! Strict actual-client request transcripts for discovery binding failures.
use super::*;

#[tokio::test]
async fn invalid_source_binding_cannot_reach_runtime_session_reads() {
    let mut foreign = inventory();
    foreign["endpoints"][0]["endpoint"]["serviceId"] =
        json!("00000000-0000-4000-8000-000000000003");
    let mut duplicate = inventory();
    let repeated = duplicate["endpoints"][0].clone();
    duplicate["endpoints"]
        .as_array_mut()
        .unwrap()
        .push(repeated);
    for invalid in [foreign, duplicate] {
        let (mut client, peer) = connect_fixture(vec![("endpoint/list", invalid)]).await;
        let result = load_runtime_records(&mut client, &[], false).await;
        let _closed = client.close().await;
        assert!(result.is_err());
        assert_eq!(peer.await.unwrap().len(), 1, "reject before sessionList");
    }
}

#[tokio::test]
async fn ambiguous_native_attribution_cannot_stamp_default_catalog_rows() {
    let mut ambiguous = inventory();
    let mut second = ambiguous["endpoints"][0].clone();
    second["endpoint"]["endpointId"] = json!("codex-other");
    ambiguous["endpoints"].as_array_mut().unwrap().push(second);
    let (mut client, peer) = connect_fixture(vec![("endpoint/list", ambiguous)]).await;
    let result = load_provider_records(&mut client).await;
    client.close().await.unwrap();
    assert!(
        result.is_err(),
        "an arbitrary native endpoint cannot attribute stored rows"
    );
    assert_eq!(peer.await.unwrap().len(), 1);
}

#[tokio::test]
async fn provider_only_inventory_remains_readable_without_native_attribution() {
    let mut providers = inventory_with_provider();
    providers["endpoints"].as_array_mut().unwrap().remove(0);
    let provider_endpoint = providers["endpoints"][0]["endpoint"].clone();
    let (mut client, peer) = connect_fixture(vec![
        ("endpoint/list", providers),
        (
            "provider/sessionList",
            json!({
                "endpoint":provider_endpoint,"observedAt":"2026-10-06T00:00:00Z",
                "sessions":[],"nextCursor":null
            }),
        ),
    ])
    .await;
    let (attribution, records) = load_provider_records(&mut client).await.unwrap();
    client.close().await.unwrap();
    assert!(attribution.is_none());
    assert!(records.is_empty());
    assert_eq!(peer.await.unwrap().len(), 2);
}
