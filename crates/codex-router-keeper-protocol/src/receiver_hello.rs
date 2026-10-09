use crate::{ComponentFingerprint, ComponentKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ReceiverHelloWire {
    Hello {
        parent_role: ComponentKind,
        fingerprint: ComponentFingerprint,
    },
}
