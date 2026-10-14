//! Source-scoped read requests and closed outcomes for one picker query generation.
use super::SessionsPickerDataQuery;
use super::source_inventory_binding::SourceInventoryBinding;
use crate::sessions::configured_inventory_loader::StoredSourceContinuation;
use crate::{
    picker_runtime_status::PickerRecordsSnapshot,
    sessions::router_connection_registry::RouterConnectionProfile,
};
use futures_util::future::BoxFuture;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PickerSourceContext {
    DefaultHosted,
    LocalCodex,
    ConfiguredHosted(RouterConnectionProfile),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceInventoryRequest {
    pub(crate) source_context: PickerSourceContext,
    pub(crate) query: SessionsPickerDataQuery,
    pub(crate) request_generation: u64,
    pub(crate) continuation: Option<SourceContinuation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum SourceInventoryRejection {
    #[error("session source is unavailable")]
    SourceUnavailable,
    #[error("source connection needs qualification")]
    EndpointUnqualified,
    #[error("session inventory does not match its source")]
    InvalidInventory,
    #[error("session inventory reply does not match its request")]
    StaleSnapshot,
    #[error("source scope or filters are unsupported; choose Any scope/provider and All sources")]
    UnsupportedViewOrScope,
    #[error("source does not support this search expression")]
    UnsupportedQuery,
    #[error("source does not provide creation times; choose Updated")]
    UnsupportedSort,
}

#[derive(Clone, Debug)]
pub(crate) enum SourceInventoryResult {
    Qualified {
        request: SourceInventoryRequest,
        binding: SourceInventoryBinding,
        continuation: SourceContinuation,
    },
    Ready {
        request: SourceInventoryRequest,
        binding: Option<SourceInventoryBinding>,
        snapshot: PickerRecordsSnapshot,
        progress: InventoryPageProgress,
    },
    Rejected {
        request: SourceInventoryRequest,
        reason: SourceInventoryRejection,
    },
}

impl SourceInventoryResult {
    pub(crate) fn request(&self) -> &SourceInventoryRequest {
        match self {
            Self::Ready { request, .. }
            | Self::Rejected { request, .. }
            | Self::Qualified { request, .. } => request,
        }
    }

    #[cfg(test)]
    pub(crate) fn into_snapshot(self) -> Result<PickerRecordsSnapshot, SourceInventoryRejection> {
        match self {
            Self::Ready { snapshot, .. } => Ok(snapshot),
            Self::Rejected { reason, .. } => Err(reason),
            Self::Qualified { .. } => Err(SourceInventoryRejection::InvalidInventory),
        }
    }
}

pub(crate) type SourceInventoryLoader =
    Arc<dyn Fn(SourceInventoryRequest) -> BoxFuture<'static, SourceInventoryResult> + Send + Sync>;

/// An invocation-owned, source-bound Stored cursor; cloning cannot replay a consumed read.
#[derive(Clone)]
pub(crate) struct SourceContinuation(Arc<tokio::sync::Mutex<Option<StoredSourceContinuation>>>);
impl SourceContinuation {
    pub(crate) fn stored(state: StoredSourceContinuation) -> Self {
        Self(Arc::new(tokio::sync::Mutex::new(Some(state))))
    }
    pub(crate) async fn take(&self) -> Option<StoredSourceContinuation> {
        self.0.lock().await.take()
    }
}
impl std::fmt::Debug for SourceContinuation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StoredSourceContinuation")
    }
}
impl PartialEq for SourceContinuation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for SourceContinuation {}

#[derive(Clone, Debug)]
pub(crate) enum InventoryPageProgress {
    Complete,
    MoreAvailable { continuation: SourceContinuation },
}
