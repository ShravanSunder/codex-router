//! Fixture bootstrap outside the core preparation behavior under test.
use codex_router_descriptor_boundary::{DescriptorGate, OwnedListener};
use codex_router_proxy::server::{
    LoopbackRouterRuntime, LoopbackRouterRuntimeConfig, LoopbackRouterRuntimeError,
};
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
pub async fn activate_core_fixture(
    config: LoopbackRouterRuntimeConfig,
    credentials: EncryptedCredentialStore,
) -> Result<LoopbackRouterRuntime, LoopbackRouterRuntimeError> {
    // External fixture setup preserves legacy seeds before the strict core transition.
    let state =
        codex_router_state::sqlite::AsyncSqliteStateStore::open(config.state_database_path())
            .await?;
    state.close().await?;
    let affinity =
        codex_router_secret_store::affinity_secret::load_or_create_router_affinity_hash_secret(
            &credentials,
        )
        .map_err(|error| LoopbackRouterRuntimeError::CredentialResources(error.into()))?
        .secret()
        .clone();
    let gate = DescriptorGate::global();
    let listener = OwnedListener::bind_tcp(config.bind_address().socket_addr(), gate).await?;
    LoopbackRouterRuntime::prepare(config, credentials, affinity, listener, gate)
        .await?
        .activate()
        .await
}

/// The old CLI fixture supplies an already-observed external credential handle.
/// Production Fresh/Replacement always use the owning secret preparation APIs.
pub async fn prepare_fresh_with_fixture_credentials(
    config: crate::ProxyRoleConfig,
    credentials: EncryptedCredentialStore,
    listener: OwnedListener,
    gate: &DescriptorGate,
) -> Result<crate::PreparedProxyRoleRuntime, crate::ProxyPreparationError> {
    let secrets = crate::proxy_secret_preparation::assemble_secrets(
        config.core.secret_store_root(),
        &codex_router_keeper_protocol::PrepareMode::Fresh,
        credentials,
    )?;
    crate::ProxyRoleRuntime::prepare_after_secrets(
        config,
        codex_router_keeper_protocol::PrepareMode::Fresh,
        listener,
        gate,
        secrets,
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
    )
    .await
}
