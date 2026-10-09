use crate::{
    credential_upkeep_worker::{
        CredentialUpkeepStartError, CredentialUpkeepWorker,
        start_background_credential_upkeep_worker,
    },
    proxy_role_config::ProxyQuotaRefreshPolicy,
    proxy_role_preparation::PreparedProxyRoleRuntime,
    quota::{
        BackgroundQuotaRefreshWorker, QuotaRefreshError, start_background_quota_refresh_worker,
    },
    token_reload_watcher::LocalTokenReloadWatcher,
};
use codex_router_auth::resolver::CredentialRefreshTaskSupervisor;
use codex_router_proxy::server::{LoopbackRouterRuntime, LoopbackRouterRuntimeError};
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
#[derive(Debug, thiserror::Error)]
pub enum ProxyActivationError {
    #[error(transparent)]
    Core(#[from] LoopbackRouterRuntimeError),
    #[error(transparent)]
    Upkeep(#[from] CredentialUpkeepStartError),
    #[error(transparent)]
    Quota(#[from] QuotaRefreshError),
    #[error("proxy startup announcement failed")]
    StartupAnnouncement(#[source] std::io::Error),
    #[error("proxy serving task failed")]
    ServingTask(#[source] tokio::task::JoinError),
}
/// Owns actual serving, token watching, quota and upkeep on the caller runtime.
pub struct ProxyRoleRuntime {
    core: Arc<LoopbackRouterRuntime>,
    stop: CancellationToken,
    serving: Option<JoinHandle<Result<usize, LoopbackRouterRuntimeError>>>,
    upkeep: Option<CredentialUpkeepWorker>,
    quota: Option<BackgroundQuotaRefreshWorker>,
    watcher: Option<LocalTokenReloadWatcher>,
}
impl PreparedProxyRoleRuntime {
    pub async fn activate(self) -> Result<ProxyRoleRuntime, ProxyActivationError> {
        self.activate_with_worker_starts(
            start_background_credential_upkeep_worker,
            start_background_quota_refresh_worker,
            |_| {},
            |_| Ok(()),
        )
        .await
    }
    pub async fn activate_with_worker_starts<U, UF, Q, QF>(
        self,
        upkeep_start: U,
        quota_start: Q,
        token_observer: impl Fn(codex_router_core::ids::TokenGeneration) + Send + 'static,
        announce: impl FnOnce(&LoopbackRouterRuntime) -> Result<(), std::io::Error>,
    ) -> Result<ProxyRoleRuntime, ProxyActivationError>
    where
        U: FnOnce(PathBuf, EncryptedCredentialStore, CredentialRefreshTaskSupervisor) -> UF,
        UF: Future<Output = Result<CredentialUpkeepWorker, CredentialUpkeepStartError>>,
        Q: FnOnce(
            PathBuf,
            PathBuf,
            EncryptedCredentialStore,
            String,
            Duration,
            codex_router_proxy::websocket::WebSocketQuotaFloorNotifier,
            CredentialRefreshTaskSupervisor,
        ) -> QF,
        QF: Future<Output = Result<BackgroundQuotaRefreshWorker, QuotaRefreshError>>,
    {
        if let crate::ProxyPreparedStateSchema::Existing(ref reported) = self.schema {
            let current = codex_router_state::sqlite::AsyncSqliteStateStore::prepare_schema(
                self.config.core.state_database_path(),
            )
            .await
            .map_err(|error| {
                ProxyActivationError::Core(LoopbackRouterRuntimeError::SchemaPreparation(error))
            })?;
            tracing::debug!(reported_schema=?reported,current_schema=?current,"revalidated proxy preparation history before migrating opener");
        }
        if let crate::ProxyPreparedStateSchema::FreshBootstrap { kind } = self.schema {
            let revalidated =
                codex_router_state::sqlite::AsyncSqliteStateStore::prepare_startup_schema(
                    self.config.core.state_database_path(),
                )
                .await
                .map_err(|error| {
                    ProxyActivationError::Core(LoopbackRouterRuntimeError::SchemaPreparation(error))
                })?;
            tracing::debug!(reported_bootstrap=?kind,current_bootstrap=?revalidated,"revalidated Fresh state before existing bootstrap migration");
            let migrated = codex_router_state::sqlite::AsyncSqliteStateStore::open(
                self.config.core.state_database_path(),
            )
            .await
            .map_err(LoopbackRouterRuntimeError::from)?;
            migrated
                .close()
                .await
                .map_err(LoopbackRouterRuntimeError::from)?;
        }
        let core = Arc::new(self.core.activate().await?);
        let refresh = core.credential_refresh_task_supervisor();
        let auth = core.local_auth_reloader();
        let mut runtime = ProxyRoleRuntime {
            core: core.clone(),
            stop: CancellationToken::new(),
            serving: None,
            upkeep: None,
            quota: None,
            watcher: Some(LocalTokenReloadWatcher::start(
                self.secrets.local_token_store,
                self.secrets.local_token.generation(),
                move |record| {
                    let generation = record.current_generation();
                    auth.reload_auth(record);
                    token_observer(generation);
                },
            )),
        };
        if let Err(error) = announce(&runtime.core) {
            runtime.shutdown().await?;
            return Err(ProxyActivationError::StartupAnnouncement(error));
        }
        let upkeep = upkeep_start(
            self.config.core.state_database_path().to_path_buf(),
            self.secrets.credentials.clone(),
            refresh.clone(),
        )
        .await;
        match upkeep {
            Ok(worker) => runtime.upkeep = Some(worker),
            Err(error) => {
                runtime.shutdown().await?;
                return Err(error.into());
            }
        }
        if self.config.quota_refresh == ProxyQuotaRefreshPolicy::Enabled {
            let quota = quota_start(
                self.config.core.state_database_path().to_path_buf(),
                self.config.core.secret_store_root().to_path_buf(),
                self.secrets.credentials,
                codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned(),
                self.config.quota_refresh_interval,
                core.websocket_quota_floor_notifier(),
                refresh,
            )
            .await;
            match quota {
                Ok(worker) => runtime.quota = Some(worker),
                Err(error) => {
                    runtime.shutdown().await?;
                    return Err(error.into());
                }
            }
        }
        let stop = runtime.stop.clone();
        let max_connections = self.config.max_connections;
        runtime.serving = Some(tokio::spawn(async move {
            core.serve_owned_protocol_connections_until_cancelled(max_connections, stop)
                .await
        }));
        Ok(runtime)
    }
}
impl ProxyRoleRuntime {
    #[must_use]
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.core.local_addr()
    }
    #[must_use]
    pub fn core(&self) -> &LoopbackRouterRuntime {
        &self.core
    }
    #[must_use]
    pub fn credential_refresh_task_supervisor(&self) -> CredentialRefreshTaskSupervisor {
        self.core.credential_refresh_task_supervisor()
    }
    pub async fn wait_serving(&mut self) -> Result<usize, ProxyActivationError> {
        let result = match self.serving.as_mut() {
            Some(task) => task
                .await
                .map_err(ProxyActivationError::ServingTask)
                .and_then(|result| result.map_err(Into::into)),
            None => Ok(0),
        };
        self.serving = None;
        result
    }
    /// Retains handles across a dropped wait; a resumed call joins the same acquired work.
    pub async fn shutdown(&mut self) -> Result<(), ProxyActivationError> {
        self.stop.cancel();
        if let Some(worker) = self.quota.as_mut() {
            worker.shutdown().await;
        }
        self.quota = None;
        if let Some(worker) = self.upkeep.as_mut() {
            worker.shutdown().await;
        }
        self.upkeep = None;
        if let Some(watcher) = self.watcher.as_mut() {
            watcher.shutdown().await;
        }
        self.watcher = None;
        let result = match self.serving.as_mut() {
            Some(task) => task
                .await
                .map_err(ProxyActivationError::ServingTask)
                .and_then(|result| result.map(|_| ()).map_err(Into::into)),
            None => Ok(()),
        };
        self.serving = None;
        self.core.shutdown().await;
        result
    }
}
impl Drop for ProxyRoleRuntime {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
