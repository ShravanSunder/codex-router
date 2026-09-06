use communication_protocol::protocol_type_schemas;
use serde_json::json;

#[test]
fn endpoint_schema_matches_runtime_ascii_identity_boundaries() {
    let schema = serde_json::to_value(schemars::schema_for!(communication_protocol::EndpointId))
        .unwrap_or_else(|error| panic!("schema: {error}"));
    let validator =
        jsonschema::validator_for(&schema).unwrap_or_else(|error| panic!("validator: {error}"));
    for name in [
        "codex-local".to_owned(),
        "codex-local\n".to_owned(),
        "Codex".to_owned(),
        "1codex".to_owned(),
        "".to_owned(),
        "a".repeat(64),
        "a".repeat(65),
        "a_".to_owned(),
    ] {
        let accepted = communication_protocol::EndpointId::try_from(name.clone()).is_ok();
        assert_eq!(
            validator.is_valid(&json!(name)),
            accepted,
            "identity {name:?}"
        );
    }
}

#[test]
fn journal_schema_rejects_unsafe_positions_and_unknown_fields() {
    let schemas = protocol_type_schemas().unwrap_or_else(|error| panic!("schemas: {error}"));
    let schema = schemas
        .get("JournalPage")
        .unwrap_or_else(|| panic!("journal schema"));
    let validator =
        jsonschema::validator_for(schema).unwrap_or_else(|error| panic!("validator: {error}"));
    let id = "00000000-0000-4000-8000-000000000001";
    let mut page = json!({"bounds":{"journalId":id,"earliestSequence":1,"lastSequence":0},"records":[],"next":{"journalId":id,"sequence":0},"caughtUp":true});
    assert!(validator.is_valid(&page));
    page["bounds"]["lastSequence"] = json!(9007199254740992_u64);
    assert!(!validator.is_valid(&page));
    page["bounds"]["lastSequence"] = json!(0);
    page["surprise"] = json!(true);
    assert!(!validator.is_valid(&page));
}

#[test]
fn snapshot_schema_preserves_required_nullable_coverage_fields() {
    let schemas = protocol_type_schemas().unwrap_or_else(|error| panic!("schemas: {error}"));
    let schema = schemas
        .get("AddressPage")
        .unwrap_or_else(|| panic!("address schema"));
    let validator =
        jsonschema::validator_for(schema).unwrap_or_else(|error| panic!("validator: {error}"));
    let id = "00000000-0000-4000-8000-000000000001";
    let mut snapshot = json!({"snapshotId":id,"capturedAt":"2026-09-05T12:00:00Z","watermark":{"journalId":id,"sequence":0},"coverage":{"endpoint":{"serviceId":id,"endpointId":"codex-local"},"observerId":null,"generation":null,"state":"initializing","observedAt":"2026-09-05T12:00:00Z"},"entries":[],"nextCursor":null});
    assert!(validator.is_valid(&snapshot));
    snapshot["coverage"]
        .as_object_mut()
        .unwrap_or_else(|| panic!("coverage object"))
        .remove("generation");
    assert!(!validator.is_valid(&snapshot));
}

#[test]
fn initialization_schema_distinguishes_requested_and_negotiated_versions() {
    let schemas = protocol_type_schemas().unwrap_or_else(|error| panic!("schemas: {error}"));
    let params = jsonschema::validator_for(
        schemas
            .get("ControlInitializationParams")
            .unwrap_or_else(|| panic!("params schema")),
    )
    .unwrap_or_else(|error| panic!("params validator: {error}"));
    let result = jsonschema::validator_for(
        schemas
            .get("ControlInitializationResult")
            .unwrap_or_else(|| panic!("result schema")),
    )
    .unwrap_or_else(|error| panic!("result validator: {error}"));
    assert!(params.is_valid(
        &json!({"version":{"major":2,"minor":0},"client":{"name":"future-client","version":"1"}})
    ));
    assert!(
        !params
            .is_valid(&json!({"version":{"major":1,"minor":0},"client":{"name":"","version":"1"}}))
    );
    let id = "00000000-0000-4000-8000-000000000001";
    let mut negotiated = json!({"version":{"major":1,"minor":0},"serviceId":id,"serviceEpoch":id,"controlSchemaDigest":format!("sha256:{}","a".repeat(64))});
    assert!(result.is_valid(&negotiated));
    negotiated["version"]["major"] = json!(2);
    assert!(!result.is_valid(&negotiated));
}
