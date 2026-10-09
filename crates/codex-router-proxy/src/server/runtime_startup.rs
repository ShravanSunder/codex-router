use super::*;

#[derive(Clone, Debug)]
pub(super) struct RuntimeWritableStateStores {
    pub(super) credential_state_store: AsyncSqliteStateStore,
    pub(super) db_write_state_store: AsyncSqliteStateStore,
    pub(super) maintenance_state_store: AsyncSqliteStateStore,
}

pub(super) async fn open_runtime_writable_state_stores(
    state_database_path: &Path,
) -> Result<RuntimeWritableStateStores, StateStoreError> {
    let credential_state_store = AsyncSqliteStateStore::open(state_database_path).await?;
    let db_write_state_store = AsyncSqliteStateStore::open(state_database_path).await?;
    let maintenance_state_store = AsyncSqliteStateStore::open(state_database_path).await?;
    Ok(RuntimeWritableStateStores {
        credential_state_store,
        db_write_state_store,
        maintenance_state_store,
    })
}

impl LoopbackRouterRuntime {
    /// Returns the active loopback address.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.server.local_addr()
    }

    /// Returns a small handle that can reload local auth while the runtime is serving.
    #[must_use]
    pub fn local_auth_reloader(&self) -> LocalAuthReloader {
        LocalAuthReloader {
            auth_gate: self.auth_gate.clone(),
            claude_edge_auth_gate: self.claude_edge_auth_gate.clone(),
            codex_local_token_authentication_required: self.local_model_authentication_required,
            websocket_revocations: self.websocket_revocations.clone(),
        }
    }

    /// Returns redacted WebSocket registry counters for runtime proof.
    #[must_use]
    pub fn websocket_registry_snapshot(&self) -> WebSocketRegistrySnapshot {
        self.websocket_revocations.snapshot()
    }

    /// Returns a narrow handle for reconnecting sessions whose account reached its quota floor.
    #[must_use]
    pub fn websocket_quota_floor_notifier(&self) -> WebSocketQuotaFloorNotifier {
        WebSocketQuotaFloorNotifier::new(self.websocket_revocations.clone())
    }

    /// Replaces local auth and closes WebSocket connections authenticated with old generations.
    pub fn reload_local_auth(
        &self,
        current: LocalRouterTokenRecord,
        previous: Vec<LocalRouterTokenRecord>,
    ) {
        self.local_auth_reloader()
            .reload_local_auth(current, previous);
    }
}
