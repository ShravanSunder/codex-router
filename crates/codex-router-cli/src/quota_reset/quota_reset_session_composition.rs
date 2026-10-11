//! Composition for one interactive quota reset session.

use std::future::Future;
#[cfg(feature = "quota-reset-test-harness")]
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;

use crate::presentation::quota::CreditUsageRefresher;
use codex_router_secret_store::runtime_credential_store::RuntimeCredentialStore;

use super::QuotaResetError;
use super::provider_protocol::HttpLiveQuotaResetProvider;
use super::reset_commit_service::LiveResetAuthorityReader;
use super::reset_commit_service::ResetWorkflowService;
use super::reset_session_supervisor::ProductionRedeemRequestIdFactory;
use super::reset_session_supervisor::ProductionResetClock;
use super::reset_session_supervisor::QuotaInteractiveSession;
use super::reset_session_supervisor::ResetSessionOutcome;
use super::reset_session_supervisor::ResetSessionPorts;

pub(crate) type ResetSessionRunner =
    Pin<Box<dyn Future<Output = ResetSessionOutcome> + Send + 'static>>;

pub(crate) struct InteractiveResetSession {
    pub(crate) ports: ResetSessionPorts,
    pub(crate) runner: ResetSessionRunner,
}

pub(crate) trait InteractiveResetSessionFactory: Send + Sync {
    fn create(
        &self,
        router_root: &Path,
        credential_store: RuntimeCredentialStore,
    ) -> Result<InteractiveResetSession, QuotaResetError>;

    fn credit_usage_refresher(&self, router_root: &Path) -> CreditUsageRefresher;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FixedOriginInteractiveResetSessionFactory;

impl InteractiveResetSessionFactory for FixedOriginInteractiveResetSessionFactory {
    fn create(
        &self,
        router_root: &Path,
        credential_store: RuntimeCredentialStore,
    ) -> Result<InteractiveResetSession, QuotaResetError> {
        compose_http_reset_session(
            router_root,
            credential_store,
            HttpLiveQuotaResetProvider::new()?,
        )
    }

    fn credit_usage_refresher(&self, router_root: &Path) -> CreditUsageRefresher {
        crate::quota::interactive_credit_usage_refresher(
            router_root.to_path_buf(),
            codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned(),
        )
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
    fn create(
        &self,
        router_root: &Path,
        credential_store: RuntimeCredentialStore,
    ) -> Result<InteractiveResetSession, QuotaResetError> {
        compose_http_reset_session(
            router_root,
            credential_store,
            HttpLiveQuotaResetProvider::new_loopback(self.provider_listener)?,
        )
    }

    fn credit_usage_refresher(&self, router_root: &Path) -> CreditUsageRefresher {
        crate::quota::interactive_credit_usage_refresher(
            router_root.to_path_buf(),
            format!("http://{}", self.provider_listener),
        )
    }
}

fn compose_http_reset_session(
    router_root: &Path,
    credential_store: codex_router_secret_store::runtime_credential_store::RuntimeCredentialStore,
    provider: HttpLiveQuotaResetProvider,
) -> Result<InteractiveResetSession, QuotaResetError> {
    let authority_reader =
        LiveResetAuthorityReader::new(router_root.join("state.sqlite"), credential_store);
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
