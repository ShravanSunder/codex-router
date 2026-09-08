use communication_protocol::control_schema_document;
use serde_json::json;

#[test]
fn public_schema_accepts_instruction_creation_and_rejects_non_v7_identity()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: a real public Control schema and an agent-callable creation request.
    let schema = control_schema_document(None)?;
    let validator = jsonschema::validator_for(&schema)?;
    let request = json!({"jsonrpc":"2.0","id":"create-1","method":"instruction/create","params":{"operationId":"019f0000-0000-7000-8000-000000000001","text":"Check repository"}});
    // Act / Assert: method registration and nested typed validation are both effective.
    if !validator.is_valid(&request) {
        return Err("instruction creation missing from published schema".into());
    }
    let mut invalid = request.clone();
    invalid["params"]["operationId"] = json!("019f0000-0000-4000-8000-000000000001");
    if validator.is_valid(&invalid) {
        return Err("public schema admitted non-v7 identity".into());
    }
    let mut extra = request;
    extra["params"]["unexpected"] = json!(true);
    if validator.is_valid(&extra) {
        return Err("instruction params were not closed".into());
    }
    Ok(())
}
