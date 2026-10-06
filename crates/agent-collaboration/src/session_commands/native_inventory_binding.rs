//! Bind a read inventory to its initialized service, independently of native launch eligibility.
use collaboration_client::protocol::{
    ChannelDescription, CodexGeneration, ControlInitializationResult, EndpointAvailability,
    EndpointInventory, EndpointRef, NativeSessionView, UuidIdentity,
};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(super) enum NativeBindingRejection {
    #[error("inventory belongs to another service")]
    WrongService,
    #[error("inventory epoch changed")]
    StaleSnapshot,
    #[error("inventory contains inconsistent endpoint descriptions")]
    InvalidInventory,
    #[error("native endpoint is absent")]
    EndpointUnavailable,
    #[error("more than one native endpoint matches")]
    AmbiguousEndpoint,
    #[error("native runtime observation is unavailable")]
    RuntimeUnavailable,
}

pub(super) enum NativeEndpointSelector {
    Exact(EndpointRef),
    UniqueNative,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum NativeInventoryContext {
    Stored,
    Runtime(CodexGeneration),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct NativeInventoryBinding {
    pub(super) endpoint: EndpointRef,
    pub(super) context: NativeInventoryContext,
}

pub(super) fn bind_native_inventory(
    inventory: &EndpointInventory,
    initialized: &ControlInitializationResult,
    expected_service: &UuidIdentity,
    selector: NativeEndpointSelector,
    view: NativeSessionView,
) -> Result<NativeInventoryBinding, NativeBindingRejection> {
    if initialized.service_id != *expected_service
        || inventory
            .endpoints
            .iter()
            .any(|entry| entry.endpoint.service_id != *expected_service)
        || matches!(&selector, NativeEndpointSelector::Exact(endpoint) if endpoint.service_id != *expected_service)
    {
        return Err(NativeBindingRejection::WrongService);
    }
    if inventory.service_epoch != initialized.service_epoch {
        return Err(NativeBindingRejection::StaleSnapshot);
    }
    let mut endpoints_seen = BTreeSet::new();
    if inventory.endpoints.len() > 64
        || inventory.sequence > 9_007_199_254_740_991
        || inventory.endpoints.iter().any(|entry| {
            !endpoints_seen.insert(&entry.endpoint)
                || entry
                    .channels
                    .iter()
                    .filter(|channel| matches!(channel, ChannelDescription::NativeCodex { .. }))
                    .count()
                    > 1
        })
    {
        return Err(NativeBindingRejection::InvalidInventory);
    }
    let mut candidates = inventory.endpoints.iter().filter(|entry| {
        let selector_matches = match &selector {
            NativeEndpointSelector::Exact(endpoint) => entry.endpoint == *endpoint,
            NativeEndpointSelector::UniqueNative => true,
        };
        selector_matches
            && entry
                .channels
                .iter()
                .any(|channel| matches!(channel, ChannelDescription::NativeCodex { .. }))
    });
    let entry = candidates
        .next()
        .ok_or(NativeBindingRejection::EndpointUnavailable)?;
    if candidates.next().is_some() {
        return Err(NativeBindingRejection::AmbiguousEndpoint);
    }
    let context = match view {
        // A catalog read neither connects to the native socket nor observes live generation.
        NativeSessionView::Stored => NativeInventoryContext::Stored,
        NativeSessionView::Loaded | NativeSessionView::Active => {
            if !matches!(entry.availability, EndpointAvailability::Available { .. }) {
                return Err(NativeBindingRejection::RuntimeUnavailable);
            }
            let generation = entry
                .channels
                .iter()
                .find_map(|channel| match channel {
                    ChannelDescription::NativeCodex { generation, .. } => generation.clone(),
                    _ => None,
                })
                .ok_or(NativeBindingRejection::RuntimeUnavailable)?;
            if generation.service_epoch != inventory.service_epoch {
                return Err(NativeBindingRejection::StaleSnapshot);
            }
            NativeInventoryContext::Runtime(generation)
        }
    };
    Ok(NativeInventoryBinding {
        endpoint: entry.endpoint.clone(),
        context,
    })
}

#[cfg(test)]
#[path = "native_inventory_binding_tests.rs"]
mod tests;
