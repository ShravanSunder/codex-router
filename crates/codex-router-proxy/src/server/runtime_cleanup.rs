use super::*;

pub(super) struct LoopbackServingCleanupContext {
    pub(super) handled_connections: usize,
    pub(super) handlers: JoinSet<Result<(), LoopbackRouterRuntimeError>>,
    pub(super) first_connection_error: Option<LoopbackRouterRuntimeError>,
    pub(super) accept_error: Option<LoopbackRouterRuntimeError>,
    pub(super) session_shutdown: CancellationToken,
    pub(super) affinity_record_tasks: TaskTracker,
    pub(super) caller_shutdown_requested: bool,
}

impl LoopbackRouterRuntime {
    /// Requests and joins the runtime-owned database and maintenance actors.
    pub async fn shutdown(&self) {
        self.db_write_actor.shutdown().await;
        self.maintenance_actor.shutdown().await;
    }

    pub(super) async fn finish_serving(
        &self,
        context: LoopbackServingCleanupContext,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let LoopbackServingCleanupContext {
            handled_connections,
            mut handlers,
            mut first_connection_error,
            accept_error,
            session_shutdown,
            affinity_record_tasks,
            caller_shutdown_requested,
        } = context;
        if first_connection_error.is_some() || accept_error.is_some() || caller_shutdown_requested {
            session_shutdown.cancel();
        }

        while let Some(joined) = handlers.join_next().await {
            store_connection_join_error(&mut first_connection_error, joined);
        }
        affinity_record_tasks.close();
        affinity_record_tasks.wait().await;
        if !self
            .credential_factory
            .drain_refresh_tasks(self.credential_refresh_shutdown_drain)
            .await
        {
            tracing::warn!(
                "credential refresh drain timed out; unresolved claims remain authoritative"
            );
        }
        self.shutdown().await;

        if let Some(error) = accept_error {
            return Err(error);
        }
        match first_connection_error {
            Some(error) => Err(error),
            None => Ok(handled_connections),
        }
    }
}
