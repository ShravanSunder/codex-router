//! One CLI grammar for exact session targets.
use clap::Args;
use collaboration_client::protocol::{
    EndpointId, EndpointRef, SessionId, SessionRef, UuidIdentity,
};

#[derive(Args, Clone)]
pub(crate) struct SessionTargetArguments {
    /// Exact SessionRef JSON; copy `.target` from `sessions list --json`. nativeThreadId is not accepted.
    #[arg(long, conflicts_with_all = ["endpoint", "session"])]
    pub(crate) to: Option<String>,
    #[arg(long, conflicts_with = "to")]
    pub(crate) endpoint: Option<String>,
    #[arg(long, conflicts_with = "to")]
    pub(crate) session: Option<String>,
}

pub(crate) enum ParsedSessionTarget {
    Exact(SessionRef),
    Pair {
        endpoint_id: EndpointId,
        session_id: SessionId,
    },
}

impl SessionTargetArguments {
    pub(crate) fn parse(&self) -> Result<ParsedSessionTarget, String> {
        if let Some(value) = &self.to {
            return serde_json::from_str(value)
                .map(ParsedSessionTarget::Exact)
                .map_err(|_| guidance());
        }
        match (&self.endpoint, &self.session) {
            (Some(endpoint), Some(session)) => Ok(ParsedSessionTarget::Pair {
                endpoint_id: endpoint
                    .clone()
                    .try_into()
                    .map_err(|_| "Invalid --endpoint".to_owned())?,
                session_id: session
                    .clone()
                    .try_into()
                    .map_err(|_| "Invalid --session".to_owned())?,
            }),
            _ => Err("Provide either --to SessionRef JSON or both --endpoint and --session".into()),
        }
    }
}

impl ParsedSessionTarget {
    pub(crate) fn resolve(self, service_id: &UuidIdentity) -> Result<SessionRef, String> {
        match self {
            Self::Exact(target) if &target.endpoint.service_id == service_id => Ok(target),
            Self::Exact(_) => Err("--to addresses a different collaboration service".into()),
            Self::Pair {
                endpoint_id,
                session_id,
            } => Ok(SessionRef {
                endpoint: EndpointRef {
                    service_id: service_id.clone(),
                    endpoint_id,
                },
                session_id,
            }),
        }
    }
    pub(crate) fn endpoint_id(&self) -> EndpointId {
        match self {
            Self::Exact(target) => target.endpoint.endpoint_id.clone(),
            Self::Pair { endpoint_id, .. } => endpoint_id.clone(),
        }
    }
}

fn guidance() -> String {
    "--to must be compact SessionRef JSON with endpoint.serviceId, endpoint.endpointId, and sessionId. nativeThreadId is a lifecycle address, not a SessionRef. Copy .target from sessions list --json.".into()
}
