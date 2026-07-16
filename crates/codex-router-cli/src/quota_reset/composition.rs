//! Production-only composition for one interactive quota reset session.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use super::QuotaResetError;
use super::provider::HttpLiveQuotaResetProvider;
use super::service::LiveResetAuthorityReader;
use super::service::ResetWorkflowService;
use super::supervisor::ProductionRedeemRequestIdFactory;
use super::supervisor::QuotaInteractiveSession;
use super::supervisor::ResetSessionPorts;

const RESET_INTENT_PORT_CAPACITY: usize = 8;

pub(crate) type ResetSessionRunner = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub(crate) struct ProductionResetSession {
    pub(crate) ports: ResetSessionPorts,
    pub(crate) runner: ResetSessionRunner,
}

pub(crate) fn compose_production_reset_session(
    router_root: &Path,
) -> Result<ProductionResetSession, QuotaResetError> {
    let authority_reader = LiveResetAuthorityReader::new(
        router_root.join("state.sqlite"),
        router_root.join("secrets"),
    );
    let provider = HttpLiveQuotaResetProvider::new()?;
    let service = ResetWorkflowService::new(authority_reader, provider);
    let (session, ports) = QuotaInteractiveSession::new(
        service,
        ProductionRedeemRequestIdFactory,
        RESET_INTENT_PORT_CAPACITY,
    );
    Ok(ProductionResetSession {
        ports,
        runner: Box::pin(async move {
            let _ = session.run().await;
        }),
    })
}
