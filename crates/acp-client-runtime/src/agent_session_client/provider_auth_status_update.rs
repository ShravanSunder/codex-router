//! Capture the connection-scoped Claude auth update without private fields.

use std::{collections::HashMap, sync::Arc};

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use session_event_model::session_profile_codec::decode_connection_auth_status;
use session_event_model::{ProviderAuthStatus, SessionEvent};
use tokio::sync::RwLock;

use crate::{ProviderCapabilityReport, SessionEventSink};

pub(super) struct ProviderAuthStatusHandler {
    auth_status: Arc<RwLock<ProviderAuthStatus>>,
    session_capabilities: Arc<RwLock<HashMap<String, ProviderCapabilityReport>>>,
    event_sink: Arc<dyn SessionEventSink>,
}

impl ProviderAuthStatusHandler {
    pub(super) fn new(
        auth_status: Arc<RwLock<ProviderAuthStatus>>,
        session_capabilities: Arc<RwLock<HashMap<String, ProviderCapabilityReport>>>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Self {
        Self {
            auth_status,
            session_capabilities,
            event_sink,
        }
    }
}

impl HandleDispatchFrom<Agent> for ProviderAuthStatusHandler {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        _connection: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, Error> {
        let Dispatch::Notification(notification) = message else {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        };
        if notification.method() != "_auth/status_update" {
            return Ok(Handled::No {
                message: Dispatch::Notification(notification),
                retry: false,
            });
        }
        let status = match decode_connection_auth_status(notification.params().clone()) {
            Ok(status) => status,
            Err(_) => {
                tracing::warn!("malformed provider auth status update");
                return Ok(Handled::Yes);
            }
        };
        *self.auth_status.write().await = status.clone();
        let reports = {
            let mut capabilities = self.session_capabilities.write().await;
            capabilities
                .iter_mut()
                .map(|(session_id, report)| {
                    report.auth_status = status.clone();
                    (session_id.clone(), report.to_session_model())
                })
                .collect::<Vec<_>>()
        };
        for (session_id, capabilities) in reports {
            self.event_sink
                .publish(
                    &session_id,
                    SessionEvent::CapabilitiesChanged { capabilities },
                )
                .map_err(|_| Error::internal_error())?;
        }
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ProviderAuthStatusHandler"
    }
}
