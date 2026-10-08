use serde_json::json;

#[test]
fn public_schema_accepts_instruction_creation_and_rejects_non_v7_identity()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: the instruction_create tool's input schema and an agent-callable request.
    let schema = serde_json::to_value(schemars::schema_for!(
        collaboration_protocol::InstructionCreateParams
    ))?;
    let validator = jsonschema::validator_for(&schema)?;
    let request =
        json!({"operationId":"019f0000-0000-7000-8000-000000000001","text":"Check repository"});
    // Act / Assert: method registration and nested typed validation are both effective.
    if !validator.is_valid(&request) {
        return Err("instruction creation missing from published schema".into());
    }
    let mut invalid = request.clone();
    invalid["operationId"] = json!("019f0000-0000-4000-8000-000000000001");
    if validator.is_valid(&invalid) {
        return Err("public schema admitted non-v7 identity".into());
    }
    let mut extra = request;
    extra["unexpected"] = json!(true);
    if validator.is_valid(&extra) {
        return Err("instruction params were not closed".into());
    }
    Ok(())
}
