use collaboration_protocol::{ProviderSessionListParams, ProviderSessionListResult};
use serde_json::json;

#[test]
fn provider_inventory_has_only_known_durable_and_live_fields() {
    let params: ProviderSessionListParams = serde_json::from_value(json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
        "view":"stored","scope":{"kind":"any"},"source":"all",
        "query":null,"pageSize":10,"cursor":null
    }))
    .expect("provider params");
    assert_eq!(params.page_size, 10);
    let result: ProviderSessionListResult = serde_json::from_value(json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
        "observedAt":"2026-09-26T00:00:00Z",
        "sessions":[{
            "origin":"hostedProvider",
            "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"session-1"},
            "workingDirectory":"/tmp/project","updatedAt":3,
            "state":"unloaded",
            "approver":{"kind":"human","humanId":"owner"},
            "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}
        }],"nextCursor":null
    })).expect("provider result");
    let wire = serde_json::to_value(result).expect("serialize");
    assert_eq!(wire["sessions"][0]["state"], "unloaded");
    assert_eq!(wire["sessions"][0]["origin"], "hostedProvider");
    assert!(wire["sessions"][0].get("title").is_none());
    assert!(wire["sessions"][0].get("source").is_none());
    assert!(wire["sessions"][0].get("model").is_none());
}

#[test]
fn cursor_provider_row_keeps_its_existing_wire_shape() {
    let row = json!({
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"cursor-local"},"sessionId":"cursor-one"},
        "workingDirectory":"/tmp/project","updatedAt":3,
        "state":"unloaded","approver":{"kind":"human","humanId":"owner"},
        "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}
    });
    let decoded: collaboration_protocol::ProviderSessionSummary =
        serde_json::from_value(row.clone()).expect("Cursor provider row");
    let serialized = serde_json::to_value(decoded).expect("Cursor serialization");
    assert!(serialized.get("origin").is_none());
    assert_eq!(serialized["target"], row["target"]);
    assert_eq!(serialized["state"], row["state"]);
}
