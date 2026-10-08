use crate::{
    proxy_preparation_error::ProxyPreparationError,
    proxy_role_config::*,
    proxy_role_runtime::ProxyRoleRuntime,
    proxy_secret_preparation::{ProxyPreparedSecrets, prepare_proxy_secrets},
};
use codex_router_descriptor_boundary::{DescriptorGate, OwnedListener};
use codex_router_keeper_protocol::{ChildComponent, ChildDegradation, PrepareMode};
use codex_router_proxy::server::{LoopbackRouterRuntime, PreparedLoopbackRouterRuntime};
use codex_router_state::{
    schema_preparation::AccountSchemaPreparation, sqlite::AsyncSqliteStateStore,
};
use std::time::Duration;
/// Missing Fresh state is retained explicitly; Pending never opens latest-schema queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProxyPreparedStateSchema {
    FreshBootstrap {
        kind: codex_router_state::schema_preparation::AccountBootstrapKind,
    },
    Uninitialized,
    Existing(AccountSchemaPreparation),
}
pub struct PreparedProxyRoleRuntime {
    pub(crate) core: PreparedLoopbackRouterRuntime,
    pub(crate) secrets: ProxyPreparedSecrets,
    pub(crate) config: ProxyRoleConfig,
    pub(crate) schema: ProxyPreparedStateSchema,
}
impl ProxyRoleRuntime {
    pub async fn prepare(
        config: ProxyRoleConfig,
        mode: PrepareMode,
        listener: OwnedListener,
        gate: &DescriptorGate,
    ) -> Result<PreparedProxyRoleRuntime, ProxyPreparationError> {
        if let PrepareMode::Replacement { active_degraded } = &mode
            && (active_degraded.len() > 1
                || active_degraded.iter().any(|pair| {
                    !matches!(
                        pair,
                        (
                            ChildComponent::PooledCredentials,
                            ChildDegradation::CredentialStoreUnavailable
                        )
                    )
                }))
        {
            return Err(ProxyPreparationError::InvalidParameters);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let root = config.core.secret_store_root().to_path_buf();
        let mode_for_read = mode.clone();
        let task =
            tokio::task::spawn_blocking(move || prepare_proxy_secrets(&root, &mode_for_read));
        let secrets = tokio::time::timeout_at(deadline, task)
            .await
            .map_err(|_| ProxyPreparationError::Deadline)?
            .map_err(ProxyPreparationError::CredentialTask)??;
        Self::prepare_after_secrets(config, mode, listener, gate, secrets, deadline).await
    }
    pub(crate) async fn prepare_after_secrets(
        mut config: ProxyRoleConfig,
        mode: PrepareMode,
        listener: OwnedListener,
        gate: &DescriptorGate,
        secrets: ProxyPreparedSecrets,
        deadline: tokio::time::Instant,
    ) -> Result<PreparedProxyRoleRuntime, ProxyPreparationError> {
        let schema = if matches!(mode, PrepareMode::Fresh)
            && !config
                .core
                .state_database_path()
                .try_exists()
                .map_err(ProxyPreparationError::StateInspection)?
        {
            ProxyPreparedStateSchema::Uninitialized
        } else {
            if matches!(mode, PrepareMode::Fresh) {
                let prepared = tokio::time::timeout_at(
                    deadline,
                    AsyncSqliteStateStore::prepare_startup_schema(
                        config.core.state_database_path(),
                    ),
                )
                .await
                .map_err(|_| ProxyPreparationError::Deadline)??;
                match prepared {
                    codex_router_state::schema_preparation::AccountStartupSchemaPreparation::Native{schema}=>ProxyPreparedStateSchema::Existing(schema),
                    codex_router_state::schema_preparation::AccountStartupSchemaPreparation::Bootstrap{kind}=>ProxyPreparedStateSchema::FreshBootstrap{kind},
                }
            } else {
                let prepared = tokio::time::timeout_at(
                    deadline,
                    AsyncSqliteStateStore::prepare_schema(config.core.state_database_path()),
                )
                .await
                .map_err(|_| ProxyPreparationError::Deadline)??;
                ProxyPreparedStateSchema::Existing(prepared)
            }
        };
        config.core = config.core.with_claude_edge_local_token(
            secrets.local_token.clone(),
            config.quota_refresh_interval,
        );
        if config.local_token == ProxyLocalTokenPolicy::Required {
            config.core = config
                .core
                .with_required_local_token(secrets.local_token.clone());
        }
        let core = LoopbackRouterRuntime::prepare(
            config.core.clone(),
            secrets.credentials.clone(),
            secrets.affinity.clone(),
            listener,
            gate,
        )
        .await?;
        Ok(PreparedProxyRoleRuntime {
            core,
            secrets,
            config,
            schema,
        })
    }
}
impl PreparedProxyRoleRuntime {
    /// Configuration after role-owned token and quota interval propagation.
    #[must_use]
    pub fn core_configuration(&self) -> &codex_router_proxy::server::LoopbackRouterRuntimeConfig {
        &self.config.core
    }
    #[must_use]
    pub fn state_schema(&self) -> &ProxyPreparedStateSchema {
        &self.schema
    }
    #[must_use]
    pub fn degradations(&self) -> &[(ChildComponent, ChildDegradation)] {
        &self.secrets.degraded
    }
    #[must_use]
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.core.local_addr()
    }
    #[must_use]
    pub fn credential_refresh_task_supervisor(
        &self,
    ) -> codex_router_auth::resolver::CredentialRefreshTaskSupervisor {
        self.core.credential_refresh_task_supervisor()
    }
}
