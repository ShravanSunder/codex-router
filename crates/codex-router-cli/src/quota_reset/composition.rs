//! Composition for one interactive quota reset session.

use std::future::Future;
#[cfg(feature = "quota-reset-test-harness")]
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;

use super::QuotaResetError;
use super::provider::HttpLiveQuotaResetProvider;
use super::service::LiveResetAuthorityReader;
use super::service::ResetWorkflowService;
use super::supervisor::ProductionRedeemRequestIdFactory;
use super::supervisor::ProductionResetClock;
use super::supervisor::QuotaInteractiveSession;
use super::supervisor::ResetSessionOutcome;
use super::supervisor::ResetSessionPorts;

pub(crate) type ResetSessionRunner =
    Pin<Box<dyn Future<Output = ResetSessionOutcome> + Send + 'static>>;

pub(crate) struct InteractiveResetSession {
    pub(crate) ports: ResetSessionPorts,
    pub(crate) runner: ResetSessionRunner,
}

pub(crate) trait InteractiveResetSessionFactory: Send + Sync {
    fn create(&self, router_root: &Path) -> Result<InteractiveResetSession, QuotaResetError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FixedOriginInteractiveResetSessionFactory;

impl InteractiveResetSessionFactory for FixedOriginInteractiveResetSessionFactory {
    fn create(&self, router_root: &Path) -> Result<InteractiveResetSession, QuotaResetError> {
        compose_http_reset_session(router_root, HttpLiveQuotaResetProvider::new()?)
    }
}

#[cfg(feature = "quota-reset-test-harness")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct LoopbackInteractiveResetSessionFactory {
    provider_listener: SocketAddr,
}

#[cfg(feature = "quota-reset-test-harness")]
impl LoopbackInteractiveResetSessionFactory {
    pub(crate) const fn new(provider_listener: SocketAddr) -> Self {
        Self { provider_listener }
    }
}

#[cfg(feature = "quota-reset-test-harness")]
impl InteractiveResetSessionFactory for LoopbackInteractiveResetSessionFactory {
    fn create(&self, router_root: &Path) -> Result<InteractiveResetSession, QuotaResetError> {
        compose_http_reset_session(
            router_root,
            HttpLiveQuotaResetProvider::new_loopback(self.provider_listener)?,
        )
    }
}

fn compose_http_reset_session(
    router_root: &Path,
    provider: HttpLiveQuotaResetProvider,
) -> Result<InteractiveResetSession, QuotaResetError> {
    let authority_reader = LiveResetAuthorityReader::new(
        router_root.join("state.sqlite"),
        router_root.join("secrets"),
    );
    let service = ResetWorkflowService::new(authority_reader, provider);
    let (session, ports) = QuotaInteractiveSession::new(
        service,
        ProductionRedeemRequestIdFactory,
        std::sync::Arc::new(ProductionResetClock),
    );
    Ok(InteractiveResetSession {
        ports,
        runner: Box::pin(session.run()),
    })
}
