//! Accepted execution uses the governing wire field, independently of Rust member naming.
use communication_protocol::NativeExecution;
use serde_json::json;

#[test]
fn accepted_execution_requires_native_turn_id_on_the_wire() {
    let value = json!({
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"worker"},
        "nativeTurnId":"exact-turn",
        "startedAt":"2026-09-09T00:00:00Z",
        "deadlineAt":"2026-09-09T01:00:00Z",
        "effectiveTimeoutSeconds":3600
    });
    let execution: NativeExecution =
        serde_json::from_value(value.clone()).expect("specified nativeTurnId must deserialize");
    assert_eq!(
        serde_json::to_value(execution).expect("execution serializes"),
        value
    );
    let mut legacy = value;
    let turn = legacy
        .as_object_mut()
        .expect("object")
        .remove("nativeTurnId")
        .expect("turn");
    legacy["turnId"] = turn;
    assert!(serde_json::from_value::<NativeExecution>(legacy).is_err());
}
