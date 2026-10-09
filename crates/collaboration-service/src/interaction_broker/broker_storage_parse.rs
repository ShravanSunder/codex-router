use super::{ApprovalBrokerError, ApprovalRequestRecord, ApprovalRoute};
use std::{collections::BTreeMap, path::Path};

pub(super) async fn read_approval_routes(
    path: &Path,
) -> Result<BTreeMap<String, ApprovalRoute>, ApprovalBrokerError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice::<Vec<ApprovalRoute>>(&bytes)
            .map_err(|_| ApprovalBrokerError::Unavailable)
            .map(|routes| {
                routes
                    .into_iter()
                    .map(|route| (route.thread_id.clone(), route))
                    .collect()
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(_) => Err(ApprovalBrokerError::Unavailable),
    }
}

pub(super) async fn read_approval_history(
    path: &Path,
) -> Result<Vec<ApprovalRequestRecord>, ApprovalBrokerError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice::<Vec<ApprovalRequestRecord>>(&bytes)
            .map_err(|_| ApprovalBrokerError::Unavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(ApprovalBrokerError::Unavailable),
    }
}
