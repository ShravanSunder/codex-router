//! Propagate a Router turn cancellation to pending provider permissions.

use collaboration_protocol::SessionRef;
use collaboration_service::ServiceApprovalBroker;
use std::sync::{Arc, Weak};
use tokio::sync::RwLock;

pub(super) async fn cancel_pending_approvals(
    approval_broker: &Arc<RwLock<Option<Weak<ServiceApprovalBroker>>>>,
    target: Option<&SessionRef>,
) {
    let Some(target) = target else {
        return;
    };
    let broker = approval_broker
        .read()
        .await
        .as_ref()
        .and_then(Weak::upgrade);
    if let Some(broker) = broker
        && let Err(error) = broker
            .cancel_all_for_session(target, "turn cancelled")
            .await
    {
        tracing::error!(%error, "failed to cancel provider permissions for turn");
    }
}
