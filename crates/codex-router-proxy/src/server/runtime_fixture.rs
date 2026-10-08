use super::*;
use codex_router_descriptor_boundary::{DescriptorGate, OwnedListener};
impl LoopbackRouterRuntime {
    pub(crate) async fn start_for_test(
        config: LoopbackRouterRuntimeConfig,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let credentials = test_credential_store_for_config(&config)?;
        Self::start_fixture(config, credentials, None).await
    }
    pub(crate) async fn start_for_test_with_maintenance_completion_sender(
        config: LoopbackRouterRuntimeConfig,
        sender: std::sync::mpsc::Sender<crate::maintenance_actor::MaintenanceCompletion>,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let credentials = test_credential_store_for_config(&config)?;
        Self::start_with_maintenance_completion_sender(config, credentials, sender).await
    }
    pub(crate) async fn start_with_maintenance_completion_sender(
        config: LoopbackRouterRuntimeConfig,
        credentials: EncryptedCredentialStore,
        sender: std::sync::mpsc::Sender<crate::maintenance_actor::MaintenanceCompletion>,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        Self::start_fixture(config, credentials, Some(sender)).await
    }
    pub(crate) async fn start_with_credentials_for_test(
        config: LoopbackRouterRuntimeConfig,
        credentials: EncryptedCredentialStore,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        Self::start_fixture(config, credentials, None).await
    }
    async fn start_fixture(
        config: LoopbackRouterRuntimeConfig,
        credentials: EncryptedCredentialStore,
        sender: Option<std::sync::mpsc::Sender<crate::maintenance_actor::MaintenanceCompletion>>,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        // External fixture setup preserves legacy seeds before the strict core transition.
        let state =
            codex_router_state::sqlite::AsyncSqliteStateStore::open(config.state_database_path())
                .await?;
        state.close().await?;
        let affinity =
            codex_router_secret_store::affinity_secret::load_or_create_router_affinity_hash_secret(
                &credentials,
            )
            .map_err(ProxyRuntimeCredentialResourcesOpenError::from)?
            .secret()
            .clone();
        let gate = DescriptorGate::global();
        let listener = OwnedListener::bind_tcp(config.bind_address.socket_addr(), gate).await?;
        let runtime = Self::prepare(config, credentials, affinity, listener, gate)
            .await?
            .activate()
            .await?;
        if let Some(sender) = sender {
            runtime.maintenance_actor.register_completion_sender(sender);
        }
        Ok(runtime)
    }
}
