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
            "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"session-1"},
            "workingDirectory":"/tmp/project","updatedAt":3,
            "state":"unloaded",
            "approver":{"kind":"human","humanId":"owner"},
            "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}
        }],"nextCursor":null
    })).expect("provider result");
    let wire = serde_json::to_value(result).expect("serialize");
    assert_eq!(wire["sessions"][0]["state"], "unloaded");
    assert!(wire["sessions"][0].get("title").is_none());
    assert!(wire["sessions"][0].get("source").is_none());
    assert!(wire["sessions"][0].get("model").is_none());
}
