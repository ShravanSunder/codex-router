use communication_protocol::{CodexGeneration, EndpointRef, SessionRef};
use serde_json::json;

#[test]
fn identical_native_ids_on_distinct_endpoints_remain_distinct() {
    // Arrange
    let target = |endpoint: &str| json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":endpoint},"sessionId":"native-thread"});
    // Act
    let first: SessionRef = serde_json::from_value(target("codex-local"))
        .unwrap_or_else(|error| panic!("valid reference: {error}"));
    let second: SessionRef = serde_json::from_value(target("codex-other"))
        .unwrap_or_else(|error| panic!("valid reference: {error}"));
    // Assert
    assert_ne!(first, second);
    assert_eq!(
        serde_json::to_value(first).unwrap_or_else(|error| panic!("serialize: {error}")),
        target("codex-local")
    );
}

#[test]
fn endpoint_validation_rejects_ambiguous_addresses_and_unknown_fields() {
    // Arrange / Act / Assert
    for endpoint in ["", "../codex", "Codex", "has space", "-codex"] {
        assert!(
            serde_json::from_value::<EndpointRef>(
                json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":endpoint})
            )
            .is_err()
        );
    }
    assert!(serde_json::from_value::<EndpointRef>(json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local","host":"guessed"})).is_err());
}

#[test]
fn generation_is_scoped_to_epoch_and_rejects_unsafe_numbers() {
    // Arrange
    let value =
        |epoch: &str, generation: u64| json!({"serviceEpoch":epoch,"generation":generation});
    // Act
    let first: CodexGeneration =
        serde_json::from_value(value("00000000-0000-4000-8000-000000000001", 1))
            .unwrap_or_else(|error| panic!("generation: {error}"));
    let second: CodexGeneration =
        serde_json::from_value(value("00000000-0000-4000-8000-000000000002", 1))
            .unwrap_or_else(|error| panic!("generation: {error}"));
    // Assert
    assert_ne!(first, second);
    for n in [0, 9_007_199_254_740_992] {
        assert!(
            serde_json::from_value::<CodexGeneration>(value(
                "00000000-0000-4000-8000-000000000001",
                n
            ))
            .is_err()
        );
    }
}
