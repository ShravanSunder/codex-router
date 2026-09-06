//! Canonical Control schema bytes and their derived identity, never a caller-invented digest.
use crate::{SchemaDigest, control_schema_document};
use sha2::{Digest, Sha256};

pub struct ControlSchema {
    bytes: Vec<u8>,
    digest: SchemaDigest,
    native_digest: Option<SchemaDigest>,
}
#[derive(Debug, thiserror::Error)]
pub enum ControlSchemaError {
    #[error("Control schema serialization failed")]
    Encoding(#[from] serde_json::Error),
    #[error("Control schema digest encoding failed")]
    DigestEncoding,
}
impl ControlSchema {
    pub fn generate(native_digest: Option<SchemaDigest>) -> Result<Self, ControlSchemaError> {
        let document = control_schema_document(native_digest.as_ref())?;
        let bytes = serde_json_canonicalizer::to_vec(&document)?;
        let hash = Sha256::digest(&bytes);
        let encoded = format!(
            "sha256:{}",
            hash.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let digest = encoded
            .try_into()
            .map_err(|_| ControlSchemaError::DigestEncoding)?;
        Ok(Self {
            bytes,
            digest,
            native_digest,
        })
    }
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    #[must_use]
    pub fn digest(&self) -> &SchemaDigest {
        &self.digest
    }
    #[must_use]
    pub fn native_digest(&self) -> Option<&SchemaDigest> {
        self.native_digest.as_ref()
    }
}
