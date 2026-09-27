use collaboration_protocol::{ProviderIdentity, SessionRef};
use serde_json::json;

#[test]
fn session_actor_is_byte_identical_to_legacy_session_ref() {
    let legacy = r#"{"endpoint":{"serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","endpointId":"codex-local"},"sessionId":"creator"}"#;
    let actor: ProviderIdentity = serde_json::from_str(legacy).expect("legacy SessionRef actor");
    assert!(matches!(actor, ProviderIdentity::Session(_)));
    assert_eq!(serde_json::to_string(&actor).expect("actor JSON"), legacy);
}

#[test]
fn human_actor_round_trips_and_old_reader_fails_closed_on_human_id() {
    let human = json!({"humanId":"owner"});
    let actor: ProviderIdentity = serde_json::from_value(human.clone()).expect("human actor");
    assert!(matches!(actor, ProviderIdentity::Human { .. }));
    assert_eq!(serde_json::to_value(actor).expect("actor JSON"), human);
    let old_reader = serde_json::from_value::<SessionRef>(human)
        .expect_err("old SessionRef-only reader must reject a human actor");
    assert!(old_reader.to_string().contains("humanId"));
}
