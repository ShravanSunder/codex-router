//! Provider route claims from catalog availability, loaded actors, and session records.
use crate::{ExternalProviderSupervisor, ProviderSessionActivity};
use collaboration_protocol::{EndpointAvailability, EndpointRef, SessionRef, UuidIdentity};
use collaboration_service::{
    EndpointDirectory, ProviderConversationBackend, ProviderOperationStore, RouteClaim,
    RouteUnavailableReason,
};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;

pub(crate) struct ProviderAcpRouteClaim {
    service_id: UuidIdentity,
    provider_endpoints: HashSet<EndpointRef>,
    directory: EndpointDirectory,
    supervisor: Arc<ExternalProviderSupervisor>,
    store: Arc<Mutex<ProviderOperationStore>>,
}

impl ProviderAcpRouteClaim {
    pub(crate) fn new(
        service_id: UuidIdentity,
        provider_endpoints: HashSet<EndpointRef>,
        directory: EndpointDirectory,
        supervisor: Arc<ExternalProviderSupervisor>,
        store: Arc<Mutex<ProviderOperationStore>>,
    ) -> Self {
        Self {
            service_id,
            provider_endpoints,
            directory,
            supervisor,
            store,
        }
    }

    pub(crate) async fn claim(&self, target: &SessionRef) -> RouteClaim {
        if !self.serves(target) {
            return RouteClaim::NotMine;
        }
        let endpoint = self
            .directory
            .subscribe()
            .and_then(|subscription| subscription.snapshot())
            .ok()
            .and_then(|snapshot| {
                snapshot
                    .endpoints
                    .into_iter()
                    .find(|entry| entry.endpoint == target.endpoint)
            });
        let Some(endpoint) = endpoint else {
            return unavailable(
                "provider endpoint is not registered",
                "restart the Router",
                false,
            );
        };
        match endpoint.availability {
            EndpointAvailability::Unavailable { reason, fix, .. } => {
                let reason = String::from(reason);
                // A retained binding identifies a provider that launched and later retired.
                // Configuration and startup failures have no binding for this Host lifetime.
                let retryable = self.supervisor.binding(&target.endpoint).is_some();
                return RouteClaim::Unavailable {
                    reason: RouteUnavailableReason {
                        reason,
                        fix: fix.map_or_else(|| "check providers.json".to_owned(), String::from),
                    },
                    retryable,
                };
            }
            EndpointAvailability::Unprobed => {
                return unavailable(
                    "provider has not started",
                    "wait for provider startup",
                    true,
                );
            }
            EndpointAvailability::Available { .. } => {}
        }
        let Some(runtime) = self.supervisor.runtime_for(&target.endpoint) else {
            return unavailable(
                "provider runtime is not ready",
                "wait for provider startup",
                false,
            );
        };
        match runtime
            .session_activity(String::from(target.session_id.clone()))
            .await
        {
            Ok(ProviderSessionActivity::Idle | ProviderSessionActivity::Running) => {
                RouteClaim::Holds
            }
            Ok(ProviderSessionActivity::NotLoaded) => {
                match self.store.lock().await.session_record(target).await {
                    Ok(Some(_)) => RouteClaim::CanLoad,
                    Ok(None) => RouteClaim::NotMine,
                    Err(_) => unavailable(
                        "provider session record is unavailable",
                        "retry after storage recovers",
                        true,
                    ),
                }
            }
            Err(_) => unavailable(
                "provider runtime is unavailable",
                "retry after provider restarts",
                true,
            ),
        }
    }

    pub(crate) fn serves(&self, target: &SessionRef) -> bool {
        target.endpoint.service_id == self.service_id
            && self.provider_endpoints.contains(&target.endpoint)
    }
}

fn unavailable(reason: &str, fix: &str, retryable: bool) -> RouteClaim {
    RouteClaim::Unavailable {
        reason: RouteUnavailableReason {
            reason: reason.to_owned(),
            fix: fix.to_owned(),
        },
        retryable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExternalProviderSupervisor;
    use collaboration_protocol::{
        EndpointAvailability, EndpointDescription, EndpointId, EndpointRef, NonEmptyText,
        ObservationTimestamp, SessionId, SessionRef, UuidIdentity,
    };
    use collaboration_service::{EndpointDirectory, ProviderOperationStore, RouteClaim};
    use std::sync::Arc;

    #[tokio::test]
    async fn disabled_provider_claim_preserves_catalog_reason_and_fix() {
        let root = tempfile::tempdir().expect("provider store root");
        let store = Arc::new(tokio::sync::Mutex::new(
            ProviderOperationStore::open(&root.path().join("operations.sqlite"))
                .await
                .expect("provider store"),
        ));
        let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
            .expect("service ID");
        let endpoint = EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint ID"),
        };
        let directory = EndpointDirectory::new(service_id.clone());
        directory
            .publish(EndpointDescription {
                endpoint: endpoint.clone(),
                label: NonEmptyText::try_from("Claude Code".to_owned()).expect("label"),
                availability: EndpointAvailability::Unavailable {
                    observed_at: ObservationTimestamp::try_from("2026-09-24T12:00:00Z".to_owned())
                        .expect("timestamp"),
                    reason: NonEmptyText::try_from("disabled in providers.json".to_owned())
                        .expect("reason"),
                    fix: Some(
                        NonEmptyText::try_from("enable Claude in providers.json".to_owned())
                            .expect("fix"),
                    ),
                },
                channels: Vec::new(),
            })
            .expect("endpoint publication");
        let supervisor = Arc::new(
            ExternalProviderSupervisor::new(Vec::new(), Arc::clone(&store)).expect("supervisor"),
        );
        let claim = ProviderAcpRouteClaim::new(
            service_id,
            std::iter::once(endpoint.clone()).collect(),
            directory,
            supervisor,
            store,
        );
        let target = SessionRef {
            endpoint,
            session_id: SessionId::try_from("fixture-session".to_owned()).expect("session ID"),
        };

        let result = claim.claim(&target).await;

        assert!(
            matches!(result, RouteClaim::Unavailable { retryable: false, reason }
            if reason.reason == "disabled in providers.json" && reason.fix.contains("enable Claude"))
        );
        let codex_target = SessionRef {
            endpoint: EndpointRef {
                service_id: target.endpoint.service_id.clone(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("endpoint ID"),
            },
            session_id: target.session_id,
        };
        assert!(matches!(
            claim.claim(&codex_target).await,
            RouteClaim::NotMine
        ));
    }
}
