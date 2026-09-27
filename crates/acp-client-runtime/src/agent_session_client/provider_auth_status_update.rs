//! Capture the connection-scoped Claude auth update without private fields.

use std::{collections::HashMap, sync::Arc};

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use session_event_model::session_profile_codec::decode_connection_auth_status;
use session_event_model::{ProviderAuthStatus, SessionEvent};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{ProviderCapabilityReport, SessionEventSink};

pub(super) struct ProviderAuthStatusHandler {
    auth_status: Arc<RwLock<ProviderAuthStatus>>,
    session_capabilities: Arc<RwLock<HashMap<String, ProviderCapabilityReport>>>,
    event_sink: Arc<dyn SessionEventSink>,
    shutdown: CancellationToken,
    sink_closed: CancellationToken,
}

impl ProviderAuthStatusHandler {
    pub(super) fn new(
        auth_status: Arc<RwLock<ProviderAuthStatus>>,
        session_capabilities: Arc<RwLock<HashMap<String, ProviderCapabilityReport>>>,
        event_sink: Arc<dyn SessionEventSink>,
        shutdown: CancellationToken,
        sink_closed: CancellationToken,
    ) -> Self {
        Self {
            auth_status,
            session_capabilities,
            event_sink,
            shutdown,
            sink_closed,
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
            if self
                .event_sink
                .publish(
                    &session_id,
                    SessionEvent::CapabilitiesChanged { capabilities },
                )
                .is_err()
            {
                self.sink_closed.cancel();
                self.shutdown.cancel();
                return Ok(Handled::Yes);
            }
        }
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ProviderAuthStatusHandler"
    }
}
