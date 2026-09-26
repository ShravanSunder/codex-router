//! Connection-level answer for agent requests that no implemented handler owns.

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub(super) struct ProviderKnownSessions(Arc<RwLock<HashSet<String>>>);

impl ProviderKnownSessions {
    pub(super) async fn track(&self, session_id: String) {
        self.0.write().await.insert(session_id);
    }

    pub(super) async fn forget(&self, session_id: &str) {
        self.0.write().await.remove(session_id);
    }

    async fn contains(&self, session_id: &str) -> bool {
        self.0.read().await.contains(session_id)
    }
}

pub(super) struct ProviderRequestSessionGuard {
    known_sessions: ProviderKnownSessions,
}

impl ProviderRequestSessionGuard {
    pub(super) fn new(known_sessions: ProviderKnownSessions) -> Self {
        Self { known_sessions }
    }
}

impl HandleDispatchFrom<Agent> for ProviderRequestSessionGuard {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        _connection: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, Error> {
        if let Dispatch::Request(request, responder) = message {
            let session_id = request
                .params()
                .get("sessionId")
                .and_then(serde_json::Value::as_str);
            if let Some(session_id) = session_id
                && !self.known_sessions.contains(session_id).await
            {
                responder.respond_with_error(Error::method_not_found())?;
                return Ok(Handled::Yes);
            }
            return Ok(Handled::No {
                message: Dispatch::Request(request, responder),
                retry: false,
            });
        }
        Ok(Handled::No {
            message,
            retry: false,
        })
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ProviderRequestSessionGuard"
    }
}

pub(super) struct ProviderRequestFallback;

impl HandleDispatchFrom<Agent> for ProviderRequestFallback {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        _connection: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, Error> {
        match message {
            Dispatch::Request(_, responder) => {
                responder.respond_with_error(Error::method_not_found())?;
                Ok(Handled::Yes)
            }
            Dispatch::Notification(_) | Dispatch::Response(_, _) => Ok(Handled::No {
                message,
                retry: false,
            }),
        }
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ProviderRequestFallback"
    }
}
