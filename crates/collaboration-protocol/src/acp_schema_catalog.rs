//! Pinned stable ACP authority; inventory membership never implies advertised capability.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const ACP_SCHEMA_BYTES: &[u8] =
    include_bytes!("../../../protocol-schemas/acp-protocol/stable-schema.json");
pub const ACP_SCHEMA_DIGEST: &str =
    "f71fbcb7beeae82770e9c33d1e5969999868789cacec509289331a1205816838";

#[derive(Debug, thiserror::Error)]
pub enum AcpSchemaError {
    #[error("pinned ACP schema integrity failed")]
    Integrity,
    #[error("ACP schema definition unavailable")]
    MissingDefinition,
    #[error("ACP schema compilation failed")]
    Compilation,
}
pub struct AcpSchemaCatalog {
    document: Value,
    methods: BTreeSet<String>,
    validators: BTreeMap<String, jsonschema::Validator>,
}
impl AcpSchemaCatalog {
    pub fn load() -> Result<Self, AcpSchemaError> {
        let digest = Sha256::digest(ACP_SCHEMA_BYTES)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if digest != ACP_SCHEMA_DIGEST {
            return Err(AcpSchemaError::Integrity);
        }
        let document: Value =
            serde_json::from_slice(ACP_SCHEMA_BYTES).map_err(|_| AcpSchemaError::Integrity)?;
        let definitions = document
            .get("$defs")
            .and_then(Value::as_object)
            .ok_or(AcpSchemaError::Integrity)?;
        let methods = definitions
            .values()
            .filter_map(|definition| {
                definition
                    .get("x-method")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<BTreeSet<_>>();
        if methods.len() != 42 {
            return Err(AcpSchemaError::Integrity);
        }
        Ok(Self {
            document,
            methods,
            validators: BTreeMap::new(),
        })
    }
    #[must_use]
    pub fn methods(&self) -> &BTreeSet<String> {
        &self.methods
    }

    /// Compiles lazily against the complete local definitions; network resolution is disabled.
    pub fn validate(&mut self, definition: &str, payload: &Value) -> Result<bool, AcpSchemaError> {
        if !self.validators.contains_key(definition) {
            let definitions = self
                .document
                .get("$defs")
                .ok_or(AcpSchemaError::Integrity)?;
            if definitions.get(definition).is_none()
                || !definition
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(AcpSchemaError::MissingDefinition);
            }
            let schema = json!({"$schema":self.document.get("$schema"), "$ref":format!("#/$defs/{definition}"), "$defs":definitions});
            let validator =
                jsonschema::validator_for(&schema).map_err(|_| AcpSchemaError::Compilation)?;
            self.validators.insert(definition.to_owned(), validator);
        }
        Ok(self
            .validators
            .get(definition)
            .ok_or(AcpSchemaError::MissingDefinition)?
            .is_valid(payload))
    }
}
