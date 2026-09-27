//! Capture the connection-scoped Claude auth update without private fields.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use session_event_model::session_profile_codec::decode_connection_auth_status;
use session_event_model::{ProviderAuthStatus, SessionEvent};
use tokio::sync::RwLock;

use super::{ActiveApprovalContext, approval_turn_cancellation::cancel_turn_after_event_overflow};
use crate::{
    InteractionPort, ProviderCapabilityReport, SessionEventSink,
    provider_connection_activity::ProviderConnectionActivity,
};

pub(super) struct ProviderAuthStatusHandler<P: InteractionPort> {
    auth_status: Arc<RwLock<ProviderAuthStatus>>,
    session_capabilities: Arc<RwLock<HashMap<String, ProviderCapabilityReport>>>,
    event_sink: Arc<dyn SessionEventSink>,
    connection_activity: Arc<ProviderConnectionActivity>,
    approval_contexts: Arc<Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
    interaction_port: Arc<P>,
}

impl<P: InteractionPort> ProviderAuthStatusHandler<P> {
    pub(super) fn new(
        auth_status: Arc<RwLock<ProviderAuthStatus>>,
        session_capabilities: Arc<RwLock<HashMap<String, ProviderCapabilityReport>>>,
        event_sink: Arc<dyn SessionEventSink>,
        connection_activity: Arc<ProviderConnectionActivity>,
        approval_contexts: Arc<Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
        interaction_port: Arc<P>,
    ) -> Self {
        Self {
            auth_status,
            session_capabilities,
            event_sink,
            connection_activity,
            approval_contexts,
            interaction_port,
        }
    }
}

impl<P: InteractionPort> HandleDispatchFrom<Agent> for ProviderAuthStatusHandler<P> {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        connection: ConnectionTo<Agent>,
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
                tracing::error!("provider Session event sink overflow on auth status update");
                let is_running = cancel_turn_after_event_overflow(
                    &self.connection_activity,
                    &self.approval_contexts,
                    &self.interaction_port,
                    &connection,
                    &session_id,
                )
                .await?;
                if !is_running {
                    return Err(Error::internal_error());
                }
            }
        }
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ProviderAuthStatusHandler"
    }
}
