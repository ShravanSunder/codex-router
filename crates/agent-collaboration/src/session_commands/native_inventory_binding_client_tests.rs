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
        let (mut client, peer) = connect_fixture(vec![("endpoints_list", invalid)]).await;
        let result = load_runtime_records(&mut client, &[], false).await;
        assert!(result.is_err());
        assert_eq!(peer.await.unwrap().len(), 1, "reject before sessionList");
    }
}

#[tokio::test]
async fn ambiguous_native_attribution_preserves_the_independent_provider_inventory() {
    let mut ambiguous = inventory_with_provider();
    let mut second = ambiguous["endpoints"][0].clone();
    second["endpoint"]["endpointId"] = json!("codex-other");
    ambiguous["endpoints"].as_array_mut().unwrap().push(second);
    let provider_endpoint = ambiguous["endpoints"][1]["endpoint"].clone();
    let (mut client, peer) = connect_fixture(vec![
        ("endpoints_list", ambiguous),
        (
            "provider_sessions_list",
            json!({
                "endpoint":provider_endpoint,"observedAt":"2026-10-06T00:00:00Z",
                "sessions":[],"nextCursor":null
            }),
        ),
    ])
    .await;
    let result = load_provider_records(&mut client).await;
    let (attribution, records) = result.unwrap();
    assert!(
        attribution.is_none(),
        "ambiguous native entries cannot attribute stored rows"
    );
    assert!(records.is_empty());
    assert_eq!(
        peer.await.unwrap().len(),
        2,
        "valid provider read remains independent"
    );
}

#[tokio::test]
async fn provider_only_inventory_remains_readable_without_native_attribution() {
    let mut providers = inventory_with_provider();
    providers["endpoints"].as_array_mut().unwrap().remove(0);
    let provider_endpoint = providers["endpoints"][0]["endpoint"].clone();
    let (mut client, peer) = connect_fixture(vec![
        ("endpoints_list", providers),
        (
            "provider_sessions_list",
            json!({
                "endpoint":provider_endpoint,"observedAt":"2026-10-06T00:00:00Z",
                "sessions":[{
                    "origin":"hostedProvider", "target":{"endpoint":provider_endpoint,"sessionId":"healthy-provider"},
                    "workingDirectory":"/provider/project","updatedAt":3,"state":"requiresAction",
                    "approver":{"kind":"human","humanId":"owner"},
                    "createdBy":{"endpoint":endpoint(),"sessionId":"creator"}
                }],"nextCursor":null
            }),
        ),
    ])
    .await;
    let (attribution, records) = load_provider_records(&mut client).await.unwrap();
    assert!(attribution.is_none());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, "healthy-provider");
    assert!(
        matches!(&records[0].identity, SessionPickerIdentity::HostedProvider(target) if target.endpoint.endpoint_id == "claude-local".to_owned().try_into().unwrap())
    );
    assert_eq!(records[0].runtime_status, PickerRuntimeStatus::Blocked);
    assert_eq!(peer.await.unwrap().len(), 2);
}
