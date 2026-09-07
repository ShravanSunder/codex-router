//! Deterministic schema bundle identity; collection and executable admission are separate.
use crate::schema_json_validation::parse_schema_document;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub struct NativeSchemaBundle {
    canonical_bytes: Vec<u8>,
    digest: [u8; 32],
}

#[derive(Debug, thiserror::Error)]
pub enum NativeSchemaError {
    #[error("native schema definition cannot be compiled")]
    InvalidSchema,
    #[error("schema export did not complete successfully")]
    ExportFailed,
    #[error("managed executable changed during schema export")]
    ExecutableChanged,
    #[error("managed executable identity observation failed")]
    ExecutableIdentity(#[source] crate::ExecutableIdentityError),
    #[error("schema export filesystem operation failed")]
    Filesystem(#[source] std::io::Error),
    #[error("schema export exceeds collection capacity")]
    Capacity,
    #[error("invalid schema filename or missing entrypoint")]
    InvalidDocuments,
    #[error("unresolved or nonlocal schema reference")]
    InvalidReference,
    #[error("invalid schema JSON")]
    InvalidJson(#[source] serde_json::Error),
}

impl NativeSchemaBundle {
    /// Canonicalizes a complete export after the caller has safely collected its files.
    /// Does not itself establish executable identity or typed-profile compatibility.
    pub fn from_documents(documents: BTreeMap<String, Vec<u8>>) -> Result<Self, NativeSchemaError> {
        let entrypoint = "codex_app_server_protocol.schemas.json";
        if !documents.contains_key(entrypoint) {
            return Err(NativeSchemaError::InvalidDocuments);
        }
        let mut parsed = BTreeMap::<String, Value>::new();
        for (name, bytes) in documents {
            if !name.ends_with(".json")
                || name.contains(['\\', '\0', ':'])
                || name
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(NativeSchemaError::InvalidDocuments);
            }
            let value = parse_schema_document(&bytes).map_err(NativeSchemaError::InvalidJson)?;
            if !value.is_object() {
                return Err(NativeSchemaError::InvalidDocuments);
            }
            parsed.insert(name, value);
        }
        for (name, document) in &parsed {
            validate_references(document, name, &parsed)?;
        }
        let canonical_bytes = serde_json_canonicalizer::to_vec(
            &json!({"entrypoint": entrypoint, "documents": parsed}),
        )
        .map_err(NativeSchemaError::InvalidJson)?;
        let digest = Sha256::digest(&canonical_bytes).into();
        Ok(Self {
            canonical_bytes,
            digest,
        })
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    #[must_use]
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

fn validate_references(
    value: &Value,
    document_name: &str,
    documents: &BTreeMap<String, Value>,
) -> Result<(), NativeSchemaError> {
    match value {
        Value::Object(fields) => {
            if let Some(reference) = fields.get("$ref") {
                let reference = reference
                    .as_str()
                    .ok_or(NativeSchemaError::InvalidReference)?;
                let (filename, pointer) = reference.split_once('#').unwrap_or((reference, ""));
                if filename.contains([':', '\\', '%', '?']) || filename.starts_with('/') {
                    return Err(NativeSchemaError::InvalidReference);
                }
                let destination = if filename.is_empty() {
                    document_name.to_owned()
                } else {
                    let mut parts: Vec<&str> = document_name.split('/').collect();
                    parts.pop();
                    for part in filename.split('/') {
                        match part {
                            ".." => {
                                parts.pop().ok_or(NativeSchemaError::InvalidReference)?;
                            }
                            "." => {}
                            "" => return Err(NativeSchemaError::InvalidReference),
                            other => parts.push(other),
                        }
                    }
                    parts.join("/")
                };
                let target = documents
                    .get(&destination)
                    .ok_or(NativeSchemaError::InvalidReference)?;
                if target.pointer(pointer).is_none() {
                    return Err(NativeSchemaError::InvalidReference);
                }
            }
            for child in fields.values() {
                validate_references(child, document_name, documents)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                validate_references(child, document_name, documents)?;
            }
        }
        _ => {}
    }
    Ok(())
}
