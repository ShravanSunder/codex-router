use communication_protocol::LifecycleObservation;
use serde_json::json;
#[test]
fn observation_validates_source_subject_and_scope_before_storage() {
    let mut value = json!({"observedAt":"2026-09-05T12:00:00Z","source":"observerLifecycle","scope":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":null,"observerId":"00000000-0000-4000-8000-000000000002"},"subject":{"kind":"backend"},"change":{"kind":"coverageLost"}});
    let valid: LifecycleObservation =
        serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("observation: {e}"));
    assert!(valid.validate().is_ok());
    value["source"] = json!("nativeNotification");
    let invalid: LifecycleObservation =
        serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("shape: {e}"));
    assert!(invalid.validate().is_err());
    value["message"] = json!("must not enter the journal");
    assert!(serde_json::from_value::<LifecycleObservation>(value).is_err());
}
