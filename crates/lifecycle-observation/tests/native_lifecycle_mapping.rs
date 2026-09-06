use communication_protocol::{LifecycleChange, ObservationOrdering, ObservationScope};
use lifecycle_observation::map_native_lifecycle;
use serde_json::json;
#[test]
fn mapping_keeps_only_lifecycle_facts_and_never_content() {
    let scope:ObservationScope=serde_json::from_value(json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},"observerId":"00000000-0000-4000-8000-000000000003"})).unwrap_or_else(|e|panic!("scope: {e}"));
    let at = || {
        "2026-09-05T12:00:00Z"
            .to_owned()
            .try_into()
            .unwrap_or_else(|e| panic!("time: {e}"))
    };
    let records=map_native_lifecycle(&json!({"method":"thread/started","params":{"thread":{"id":"t","status":{"type":"idle"},"preview":"SECRET-CONTENT"}}}),&scope,at(),ObservationOrdering::Ambiguous).unwrap_or_else(|e|panic!("map: {e}"));
    assert_eq!(records.len(), 2);
    assert!(matches!(
        records.first().map(|r| &r.change),
        Some(LifecycleChange::ThreadDiscovered)
    ));
    assert!(
        !serde_json::to_string(&records)
            .unwrap_or_else(|e| panic!("serialize: {e}"))
            .contains("SECRET-CONTENT")
    );
    assert!(
        map_native_lifecycle(
            &json!({"method":"item/agentMessage/delta","params":{"delta":"SECRET-CONTENT"}}),
            &scope,
            at(),
            ObservationOrdering::Established
        )
        .unwrap_or_else(|e| panic!("ignore: {e}"))
        .is_empty()
    );
    assert!(map_native_lifecycle(&json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"turn","status":"inProgress"}}}),&scope,at(),ObservationOrdering::Established).is_err());
}
