//! Endpoint snapshot and notification contracts with connection-local ordering.
use crate::{EndpointDescription, UuidIdentity};
use serde::{Deserialize, Serialize};
#[derive(schemars::JsonSchema, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndpointInventory {
    pub service_epoch: UuidIdentity,
    #[schemars(range(min = 0, max = 9007199254740991_u64))]
    pub sequence: u64,
    #[schemars(length(max = 64))]
    pub endpoints: Vec<EndpointDescription>,
}
#[derive(schemars::JsonSchema, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndpointChange {
    pub service_epoch: UuidIdentity,
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub sequence: u64,
    pub endpoint: EndpointDescription,
}
