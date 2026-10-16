//! Observed machine inventory membership, independent of native launch eligibility.
use super::source_inventory_request::SourceInventoryRejection;
use collaboration_client::protocol::{
    ChannelDescription, EndpointInventory, EndpointRef, UuidIdentity,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InventoryEndpointKind {
    NativeCodex,
    ExternalProvider,
}

#[cfg(test)]
#[path = "source_inventory_binding_tests.rs"]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceInventoryBinding {
    pub(crate) service_id: UuidIdentity,
    pub(crate) service_epoch: UuidIdentity,
    pub(crate) endpoints: BTreeMap<EndpointRef, InventoryEndpointKind>,
}

impl SourceInventoryBinding {
    pub(crate) fn from_inventory(
        expected_service: &UuidIdentity,
        inventory: &EndpointInventory,
    ) -> Result<Self, SourceInventoryRejection> {
        let first = inventory
            .endpoints
            .first()
            .ok_or(SourceInventoryRejection::EndpointUnqualified)?;
        if first.endpoint.service_id != *expected_service
            || inventory.endpoints.len() > 64
            || inventory.sequence > 9_007_199_254_740_991
        {
            return Err(SourceInventoryRejection::InvalidInventory);
        }
        let mut seen = BTreeSet::new();
        let mut endpoints = BTreeMap::new();
        for description in &inventory.endpoints {
            if description.endpoint.service_id != *expected_service
                || !seen.insert(description.endpoint.clone())
            {
                return Err(SourceInventoryRejection::InvalidInventory);
            }
            let mut kind = None;
            for channel in &description.channels {
                let candidate = match channel {
                    ChannelDescription::NativeCodex { .. } => {
                        Some(InventoryEndpointKind::NativeCodex)
                    }
                    ChannelDescription::ExternalProvider { .. } => {
                        Some(InventoryEndpointKind::ExternalProvider)
                    }
                    ChannelDescription::Acp { .. } => None,
                };
                if let Some(candidate) = candidate
                    && kind.replace(candidate).is_some()
                {
                    return Err(SourceInventoryRejection::InvalidInventory);
                }
            }
            if let Some(kind) = kind {
                endpoints.insert(description.endpoint.clone(), kind);
            }
        }
        if endpoints.is_empty() {
            return Err(SourceInventoryRejection::EndpointUnqualified);
        }
        Ok(Self {
            service_id: first.endpoint.service_id.clone(),
            service_epoch: inventory.service_epoch.clone(),
            endpoints,
        })
    }

    pub(crate) fn contains(&self, endpoint: &EndpointRef, kind: InventoryEndpointKind) -> bool {
        endpoint.service_id == self.service_id && self.endpoints.get(endpoint) == Some(&kind)
    }

    /// A declared fixture membership stands in for the producer in picker-only tests.
    #[cfg(test)]
    pub(crate) fn native_fixture(endpoint: EndpointRef) -> Self {
        Self {
            service_id: endpoint.service_id.clone(),
            service_epoch: "00000000-0000-4000-8000-000000000002"
                .to_owned()
                .try_into()
                .unwrap(),
            endpoints: BTreeMap::from([(endpoint, InventoryEndpointKind::NativeCodex)]),
        }
    }
}
