use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ComponentKind {
    Keeper,
    AgentCollaborationServices,
    AgentProxyServices,
    AgentProviderServices,
}
