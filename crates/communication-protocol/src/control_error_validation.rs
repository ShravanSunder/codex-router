//! Validate method-specific error envelopes from the single published Control schema owner.
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

static ERROR_VALIDATORS: OnceLock<Option<BTreeMap<String, jsonschema::Validator>>> =
    OnceLock::new();

/// Error shapes are independent of native payload profiles. Unknown methods fail closed.
#[must_use]
pub fn control_error_is_valid(method: &str, frame: &Value) -> bool {
    let Some(error) = frame.get("error") else {
        return false;
    };
    // JSON Schema maxLength counts characters; Control additionally bounds UTF-8 bytes.
    if !bounded_text(error.get("message"), 1024)
        || frame
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.len() > 128)
    {
        return false;
    }
    if error.get("code").and_then(Value::as_i64) == Some(-32050)
        && let Some(data) = error.get("data")
        && ((data.get("message").is_some() && !bounded_text(data.get("message"), 1024))
            || (data.get("clientUserMessageId").is_some()
                && !bounded_text(data.get("clientUserMessageId"), 4096)))
    {
        return false;
    }
    ERROR_VALIDATORS
        .get_or_init(compile_error_validators)
        .as_ref()
        .and_then(|validators| validators.get(method))
        .is_some_and(|validator| validator.is_valid(frame))
}

fn bounded_text(value: Option<&Value>, maximum: usize) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty() && text.len() <= maximum)
}

fn compile_error_validators() -> Option<BTreeMap<String, jsonschema::Validator>> {
    let document = crate::control_schema_document(None).ok()?;
    let methods = document.get("x-methods")?.as_object()?;
    let mut validators = BTreeMap::new();
    for (method, contract) in methods {
        let reference = contract.get("error")?.get("$ref")?.as_str()?;
        let schema = json!({"$schema":document.get("$schema")?,"$id":document.get("$id")?,"$defs":document.get("$defs")?,"$ref":reference});
        // jsonschema is built without network retrieval; all error references are local.
        validators.insert(method.clone(), jsonschema::validator_for(&schema).ok()?);
    }
    Some(validators)
}
