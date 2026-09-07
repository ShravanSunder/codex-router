use codex_acp_adapter::{AcpNegotiation, AcpSchemaCatalog};
use serde_json::json;

#[test]
fn negotiation_preserves_integer_ids_and_advertises_only_selected_capabilities() {
    let mut catalog = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("schema: {error}"));
    let mut negotiation = AcpNegotiation::default();
    let request = json!({"jsonrpc":"2.0","id":9223372036854775807_i64,"method":"initialize","params":{"protocolVersion":1}});
    let response = negotiation
        .initialize(&mut catalog, &request)
        .unwrap_or_else(|error| panic!("initialize: {error}"));
    assert_eq!(response["id"], request["id"]);
    assert_eq!(response["result"]["protocolVersion"], 1);
    assert!(
        catalog
            .validate("InitializeResponse", &response["result"])
            .unwrap_or_else(|error| panic!("result schema: {error}"))
    );
    assert_eq!(response["result"]["agentCapabilities"]["loadSession"], true);
    assert_eq!(
        response["result"]["agentCapabilities"]["promptCapabilities"]["image"],
        false
    );
    assert!(negotiation.is_initialized());
    assert!(negotiation.initialize(&mut catalog, &request).is_err());
}

#[test]
fn higher_client_version_negotiates_supported_version_for_client_to_accept() {
    let mut catalog = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("schema: {error}"));
    let mut negotiation = AcpNegotiation::default();
    let response = negotiation.initialize(&mut catalog, &json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":2}}))
        .unwrap_or_else(|error| panic!("initialize: {error}"));
    assert_eq!(response["result"]["protocolVersion"], 1);
    assert!(negotiation.is_initialized());
}
