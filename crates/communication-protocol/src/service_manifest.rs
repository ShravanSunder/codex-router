//! Versioned owner-local service discovery contract.
use crate::{SchemaDigest, UuidIdentity};
use serde::{Deserialize, Serialize};

#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ControlTransport {
    #[serde(rename = "unixJsonLines")]
    UnixJsonLines,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ControlSocketPath {
    #[serde(rename = "control.sock")]
    ControlSocket,
}
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlSelector {
    pub transport: ControlTransport,
    pub path: ControlSocketPath,
}
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceManifest {
    #[serde(deserialize_with = "read_version")]
    #[schemars(range(min = 1, max = 1))]
    pub version: u8,
    pub service_id: UuidIdentity,
    pub service_epoch: UuidIdentity,
    pub control: ControlSelector,
    pub control_schema_digest: SchemaDigest,
}
fn read_version<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let version = u8::deserialize(deserializer)?;
    if version != 1 {
        return Err(serde::de::Error::custom("unsupported manifest version"));
    }
    Ok(version)
}
