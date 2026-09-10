//! Validate method-specific error envelopes from the single published Control schema owner.
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

static ERROR_VALIDATORS: OnceLock<Option<ErrorValidatorCatalog>> = OnceLock::new();

struct ErrorValidatorCatalog {
    document: Value,
    methods: BTreeMap<String, MethodErrorValidator>,
}

struct MethodErrorValidator {
    schema_reference: String,
    compiled: OnceLock<Option<jsonschema::Validator>>,
}

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
        .get_or_init(ErrorValidatorCatalog::load)
        .as_ref()
        .and_then(|catalog| catalog.validator(method))
        .is_some_and(|validator| validator.is_valid(frame))
}

fn bounded_text(value: Option<&Value>, maximum: usize) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty() && text.len() <= maximum)
}

impl ErrorValidatorCatalog {
    fn load() -> Option<Self> {
        let document = crate::control_schema_document(None).ok()?;
        let contracts = document.get("x-methods")?.as_object()?;
        let mut methods = BTreeMap::new();
        for (method, contract) in contracts {
            methods.insert(
                method.clone(),
                MethodErrorValidator {
                    schema_reference: contract.get("error")?.get("$ref")?.as_str()?.to_owned(),
                    compiled: OnceLock::new(),
                },
            );
        }
        Some(Self { document, methods })
    }

    fn validator(&self, method: &str) -> Option<&jsonschema::Validator> {
        let entry = self.methods.get(method)?;
        // A native failure must not wait for unrelated automation error schemas.
        // Each method still validates against the same complete published document.
        entry
            .compiled
            .get_or_init(|| {
                let schema = json!({
                    "$schema": self.document.get("$schema")?,
                    "$id": self.document.get("$id")?,
                    "$defs": self.document.get("$defs")?,
                    "$ref": entry.schema_reference,
                });
                // Network retrieval is disabled; all error references are local.
                jsonschema::validator_for(&schema).ok()
            })
            .as_ref()
    }
}
