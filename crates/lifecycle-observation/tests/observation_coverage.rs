use communication_protocol::{LifecycleObservation, ObservationScope};
use lifecycle_observation::ObservationCoverage;
use serde_json::json;
#[test]
fn fresh_status_requires_exact_live_scope_and_restarts_do_not_inherit_it() {
    let scope:ObservationScope=serde_json::from_value(json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000003","generation":1},"observerId":"00000000-0000-4000-8000-000000000002"})).unwrap_or_else(|e|panic!("scope: {e}"));
    let observation = |kind| {
        serde_json::from_value::<LifecycleObservation>(json!({"observedAt":"2026-09-05T12:00:00Z","source":"observerLifecycle","scope":scope,"subject":{"kind":"backend"},"change":{"kind":kind}})).unwrap_or_else(|e|panic!("record: {e}"))
    };
    let mut coverage = ObservationCoverage::default();
    assert!(!coverage.is_observing(&scope));
    coverage
        .apply(&observation("coverageRestored"))
        .unwrap_or_else(|e| panic!("restore: {e}"));
    assert!(coverage.is_observing(&scope));
    let mut old = scope.clone();
    old.observer_id = "00000000-0000-4000-8000-000000000099"
        .to_owned()
        .try_into()
        .unwrap_or_else(|e| panic!("id: {e}"));
    assert!(!coverage.is_observing(&old));
    coverage
        .apply(&observation("coverageLost"))
        .unwrap_or_else(|e| panic!("loss: {e}"));
    assert!(!coverage.is_observing(&scope));
    assert!(!ObservationCoverage::default().is_observing(&scope));
}
