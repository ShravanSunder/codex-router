use codex_acp_adapter::AcpSchemaCatalog;
use serde_json::json;

#[test]
fn pinned_schema_inventory_and_real_prompt_shapes_are_validated_offline() {
    let mut catalog = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
    assert_eq!(catalog.methods().len(), 42);
    assert!(catalog.methods().contains("session/prompt"));
    assert!(
        catalog
            .validate(
                "PromptRequest",
                &json!({"sessionId":"thread","prompt":[{"type":"text","text":"hello"}]})
            )
            .unwrap_or_else(|error| panic!("prompt schema: {error}"))
    );
    assert!(
        !catalog
            .validate(
                "PromptRequest",
                &json!({"sessionId":"thread","prompt":[{"type":"text","text":42}]})
            )
            .unwrap_or_else(|error| panic!("invalid prompt schema: {error}"))
    );
    assert!(catalog.validate("MissingDefinition", &json!({})).is_err());
}
