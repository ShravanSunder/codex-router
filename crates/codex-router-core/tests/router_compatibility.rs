use codex_router_core::router_compatibility::ROUTER_COMPATIBILITY_REVISION;
use codex_router_core::router_compatibility::RouterCompatibility;

#[test]
fn compatibility_payload_exposes_only_the_static_host_contract() {
    let payload = RouterCompatibility::current(false);
    let value = serde_json::to_value(payload)
        .unwrap_or_else(|error| panic!("compatibility payload should serialize: {error}"));
    let fields = value
        .as_object()
        .unwrap_or_else(|| panic!("compatibility payload should be a JSON object"));

    assert_eq!(fields.len(), 4);
    assert_eq!(
        fields.get("product"),
        Some(&serde_json::json!("codex-router"))
    );
    assert_eq!(
        fields.get("compatibility_revision"),
        Some(&serde_json::json!(ROUTER_COMPATIBILITY_REVISION))
    );
    assert_eq!(
        fields.get("binary_version"),
        Some(&serde_json::json!(env!("CARGO_PKG_VERSION")))
    );
    assert_eq!(
        fields.get("local_model_authentication_required"),
        Some(&serde_json::json!(false))
    );
}

#[test]
fn compatibility_payload_reports_local_model_authentication_without_credentials() {
    let rendered = serde_json::to_string(&RouterCompatibility::current(true))
        .unwrap_or_else(|error| panic!("compatibility payload should serialize: {error}"));

    assert!(rendered.contains("\"local_model_authentication_required\":true"));
    assert!(!rendered.contains("token"));
    assert!(!rendered.contains("credential"));
    assert!(!rendered.contains("account"));
    assert!(!rendered.contains("session"));
}
