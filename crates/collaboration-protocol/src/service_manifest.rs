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
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum McpTransport {
    #[serde(rename = "streamableHttp")]
    StreamableHttp,
}
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpSelector {
    pub transport: McpTransport,
    pub url: String,
}
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceManifest {
    #[serde(deserialize_with = "read_version")]
    #[schemars(range(min = 2, max = 2))]
    pub version: u8,
    pub service_id: UuidIdentity,
    pub service_epoch: UuidIdentity,
    pub control: ControlSelector,
    pub control_schema_digest: SchemaDigest,
    pub mcp: McpSelector,
}
fn read_version<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let version = u8::deserialize(deserializer)?;
    if version != 2 {
        return Err(serde::de::Error::custom("unsupported manifest version"));
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::ServiceManifest;
    use serde_json::json;

    fn manifest(version: u8) -> serde_json::Value {
        json!({
            "version":version,
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002",
            "control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":format!("sha256:{}", "a".repeat(64)),
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:8788/mcp"}
        })
    }

    #[test]
    fn strict_v2_manifest_rejects_version_skew() {
        assert!(serde_json::from_value::<ServiceManifest>(manifest(2)).is_ok());
        let error = serde_json::from_value::<ServiceManifest>(manifest(1))
            .expect_err("version one must be rejected")
            .to_string();
        assert!(error.contains("unsupported manifest version"));
    }
}
