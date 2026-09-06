//! Payload validation against an exported native definition, independent of semantic admission.
use crate::{NativeSchemaBundle, NativeSchemaError};
use serde_json::Value;

pub struct NativeSchemaValidator {
    validator: jsonschema::Validator,
}

impl NativeSchemaBundle {
    /// Resolves an exact v2 definition in this bundle. This alone does not admit a typed adapter.
    pub fn validator_for_v2(
        &self,
        definition: &str,
    ) -> Result<NativeSchemaValidator, NativeSchemaError> {
        self.validator_for_path(Some("v2"), definition)
    }
    /// Validates a generated top-level envelope such as ServerRequest or ServerNotification.
    pub fn validator_for_root(
        &self,
        definition: &str,
    ) -> Result<NativeSchemaValidator, NativeSchemaError> {
        self.validator_for_path(None, definition)
    }
    fn validator_for_path(
        &self,
        namespace: Option<&str>,
        definition: &str,
    ) -> Result<NativeSchemaValidator, NativeSchemaError> {
        if definition.is_empty()
            || !definition
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(NativeSchemaError::InvalidReference);
        }
        let bundle: Value = serde_json::from_slice(self.canonical_bytes())
            .map_err(NativeSchemaError::InvalidJson)?;
        let entrypoint = bundle
            .get("documents")
            .and_then(|documents| documents.get("codex_app_server_protocol.schemas.json"))
            .ok_or(NativeSchemaError::InvalidDocuments)?;
        let definitions = entrypoint
            .get("definitions")
            .ok_or(NativeSchemaError::InvalidDocuments)?;
        let types = match namespace {
            Some(namespace) => definitions
                .get(namespace)
                .ok_or(NativeSchemaError::InvalidReference)?,
            None => definitions,
        };
        if types.get(definition).is_none() {
            return Err(NativeSchemaError::InvalidReference);
        }
        let reference = match namespace {
            Some(namespace) => format!("#/definitions/{namespace}/{definition}"),
            None => format!("#/definitions/{definition}"),
        };
        let schema = serde_json::json!({
            "$schema":"http://json-schema.org/draft-07/schema#",
            "$ref":reference,
            "definitions":definitions,
        });
        let validator =
            jsonschema::draft7::new(&schema).map_err(|_| NativeSchemaError::InvalidSchema)?;
        Ok(NativeSchemaValidator { validator })
    }
}
impl NativeSchemaValidator {
    #[must_use]
    pub fn is_valid(&self, payload: &Value) -> bool {
        self.validator.is_valid(payload)
    }
}
