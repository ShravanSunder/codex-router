//! Source-scoped read requests and closed outcomes for one picker query generation.
use super::SessionsPickerDataQuery;
use crate::{
    picker_runtime_status::PickerRecordsSnapshot,
    sessions::router_connection_registry::RouterConnectionProfile,
};
use collaboration_client::protocol::EndpointRef;
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
    #[error("source scope or provider filter is unsupported; choose Any")]
    UnsupportedViewOrScope,
    #[error("source does not support this search expression")]
    UnsupportedQuery,
}

#[derive(Clone, Debug)]
pub(crate) enum SourceInventoryResult {
    Ready {
        request: SourceInventoryRequest,
        bound_endpoint: Option<EndpointRef>,
        snapshot: PickerRecordsSnapshot,
    },
    Rejected {
        request: SourceInventoryRequest,
        reason: SourceInventoryRejection,
    },
}

impl SourceInventoryResult {
    pub(crate) fn bound_endpoint(&self) -> Option<&EndpointRef> {
        match self {
            Self::Ready { bound_endpoint, .. } => bound_endpoint.as_ref(),
            Self::Rejected { .. } => None,
        }
    }
    pub(crate) fn request(&self) -> &SourceInventoryRequest {
        match self {
            Self::Ready { request, .. } | Self::Rejected { request, .. } => request,
        }
    }

    pub(crate) fn into_snapshot(self) -> Result<PickerRecordsSnapshot, SourceInventoryRejection> {
        match self {
            Self::Ready { snapshot, .. } => Ok(snapshot),
            Self::Rejected { reason, .. } => Err(reason),
        }
    }
}

pub(crate) type SourceInventoryLoader =
    Arc<dyn Fn(SourceInventoryRequest) -> BoxFuture<'static, SourceInventoryResult> + Send + Sync>;
