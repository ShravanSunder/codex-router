//! Logical listener association; adapter endpoint identity stays at its existing owner.
use collaboration_protocol::{EndpointId, IdentityError};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "ListenerKindWire", into = "ListenerKindWire")]
pub enum ListenerKind {
    CollaborationControl,
    NativeRelay,
    AcpChannel,
    McpHttp,
    ProxyHttp,
    RouterSessionFace { endpoint: EndpointId },
    ProviderLink,
}
// Struct variants preserve strict unknown-field refusal even for empty variants.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum ListenerKindWire {
    CollaborationControl {},
    NativeRelay {},
    AcpChannel {},
    McpHttp {},
    ProxyHttp {},
    RouterSessionFace { endpoint: String },
    ProviderLink {},
}
impl TryFrom<ListenerKindWire> for ListenerKind {
    type Error = IdentityError;
    fn try_from(wire: ListenerKindWire) -> Result<Self, Self::Error> {
        Ok(match wire {
            ListenerKindWire::CollaborationControl {} => Self::CollaborationControl,
            ListenerKindWire::NativeRelay {} => Self::NativeRelay,
            ListenerKindWire::AcpChannel {} => Self::AcpChannel,
            ListenerKindWire::McpHttp {} => Self::McpHttp,
            ListenerKindWire::ProxyHttp {} => Self::ProxyHttp,
            ListenerKindWire::RouterSessionFace { endpoint } => Self::RouterSessionFace {
                endpoint: EndpointId::try_from(endpoint)?,
            },
            ListenerKindWire::ProviderLink {} => Self::ProviderLink,
        })
    }
}
impl From<ListenerKind> for ListenerKindWire {
    fn from(kind: ListenerKind) -> Self {
        match kind {
            ListenerKind::CollaborationControl => Self::CollaborationControl {},
            ListenerKind::NativeRelay => Self::NativeRelay {},
            ListenerKind::AcpChannel => Self::AcpChannel {},
            ListenerKind::McpHttp => Self::McpHttp {},
            ListenerKind::ProxyHttp => Self::ProxyHttp {},
            ListenerKind::RouterSessionFace { endpoint } => Self::RouterSessionFace {
                endpoint: endpoint.into(),
            },
            ListenerKind::ProviderLink => Self::ProviderLink {},
        }
    }
}
