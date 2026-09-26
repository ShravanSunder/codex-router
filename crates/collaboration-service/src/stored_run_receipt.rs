//! Existing native Run acceptance JSON decodes beside the new client-neutral receipt.
use collaboration_protocol::{DeliveryReceipt, NativeSendReceipt};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum StoredRunReceipt {
    Current(DeliveryReceipt),
    LegacyNative(NativeSendReceipt),
}

impl StoredRunReceipt {
    pub(crate) fn into_public(self) -> DeliveryReceipt {
        match self {
            Self::Current(receipt) => receipt,
            Self::LegacyNative(receipt) => receipt.into(),
        }
    }
}

impl From<NativeSendReceipt> for StoredRunReceipt {
    fn from(receipt: NativeSendReceipt) -> Self {
        Self::LegacyNative(receipt)
    }
}

impl From<StoredRunReceipt> for DeliveryReceipt {
    fn from(receipt: StoredRunReceipt) -> Self {
        receipt.into_public()
    }
}
