//! Version negotiation contracts shared by the service, SDK and schema generator.
use crate::{NonEmptyText, SchemaDigest, UuidIdentity};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolVersion {
    #[schemars(range(min = 0, max = 9007199254740991_u64))]
    pub major: u64,
    #[schemars(range(min = 0, max = 9007199254740991_u64))]
    pub minor: u64,
}
#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlClientInfo {
    pub name: NonEmptyText,
    pub version: NonEmptyText,
}
#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlInitializationParams {
    pub version: ProtocolVersion,
    pub client: ControlClientInfo,
}
#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlInitializationResult {
    #[schemars(schema_with = "supported_version_schema")]
    pub version: ProtocolVersion,
    pub service_id: UuidIdentity,
    pub service_epoch: UuidIdentity,
    pub control_schema_digest: SchemaDigest,
}
fn supported_version_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"object","properties":{"major":{"const":1},"minor":{"const":0}},"required":["major","minor"],"additionalProperties":false})
}
