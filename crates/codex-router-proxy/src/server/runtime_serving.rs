use super::runtime_cleanup::LoopbackServingCleanupContext;
use super::*;

/// Receives diagnostics from detached loopback connection tasks.
pub trait LoopbackConnectionErrorReporter: Send + Sync {
    /// Reports one redacted loopback connection diagnostic.
    fn report_connection_error(&self, diagnostic: &str);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StderrLoopbackConnectionErrorReporter;

impl LoopbackConnectionErrorReporter for StderrLoopbackConnectionErrorReporter {
    fn report_connection_error(&self, diagnostic: &str) {
        // The caller renders only fixed severity, class, and reason labels.
        // Retain those details in Router telemetry when a Host captures stderr.
        tracing::warn!(
            event.name = "codex_router.proxy.loopback_connection_error",
            diagnostic = %diagnostic,
            "loopback connection failed"
        );
        eprintln!("{diagnostic}");
    }
}

#[derive(Clone, Copy)]
pub(super) enum ConnectionFailurePolicy {
    ReportAndContinue,
    StopServing,
}
impl LoopbackRouterRuntime {
    /// Role serving retains every connection task and keeps the existing unlimited error policy.
    pub async fn serve_owned_protocol_connections_until_cancelled(
        &self,
        max_connections: usize,
        shutdown: CancellationToken,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let policy = if max_connections == usize::MAX {
            ConnectionFailurePolicy::ReportAndContinue
        } else {
            ConnectionFailurePolicy::StopServing
        };
        self.serve_protocol_connections_owned(max_connections, Some(shutdown), policy)
            .with_subscriber(self.caller_dispatcher.clone())
            .await
    }

    /// Serves a bounded number of HTTP/SSE connections.
    #[cfg(test)]
    pub async fn serve_http_connections(
        &self,
        max_connections: usize,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        self.serve_protocol_connections(max_connections).await
    }

    /// Serves a bounded number of HTTP/SSE or WebSocket connections.
    pub async fn serve_protocol_connections(
        &self,
        max_connections: usize,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        tokio::runtime::Handle::try_current().map_err(|error| {
            LoopbackRouterRuntimeError::TokioRuntime(std::io::Error::other(error))
        })?;
        self.serve_protocol_connections_async(max_connections, None)
            .with_subscriber(self.caller_dispatcher.clone())
            .await
    }

    /// Serves HTTP/SSE or WebSocket connections until the bound or cancellation.
    pub async fn serve_protocol_connections_until_cancelled(
        &self,
        max_connections: usize,
        shutdown: CancellationToken,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        tokio::runtime::Handle::try_current().map_err(|error| {
            LoopbackRouterRuntimeError::TokioRuntime(std::io::Error::other(error))
        })?;
        self.serve_protocol_connections_async(max_connections, Some(shutdown))
            .with_subscriber(self.caller_dispatcher.clone())
            .await
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_connection_error_reporter(
        mut self,
        reporter: Arc<dyn LoopbackConnectionErrorReporter>,
    ) -> Self {
        self.connection_error_reporter = reporter;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_credential_refresh_shutdown_drain(mut self, limit: Duration) -> Self {
        self.credential_refresh_shutdown_drain = limit;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_test_claude_refresh_client<C>(mut self, refresh_client: C) -> Self
    where
        C: codex_router_auth::resolver::CredentialRefreshClient + Clone + Send + Sync + 'static,
    {
        self.credential_factory
            .set_test_claude_refresh_client(refresh_client);
        self
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_affinity_owner_recorder(
        mut self,
        recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    ) -> Self {
        self.affinity_owner_recorder = recorder;
        self
    }

    pub(super) async fn serve_protocol_connections_async(
        &self,
        max_connections: usize,
        shutdown: Option<CancellationToken>,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let policy = if max_connections == usize::MAX && shutdown.is_none() {
            ConnectionFailurePolicy::ReportAndContinue
        } else {
            ConnectionFailurePolicy::StopServing
        };
        self.serve_protocol_connections_owned(max_connections, shutdown, policy)
            .await
    }

    async fn serve_protocol_connections_owned(
        &self,
        max_connections: usize,
        shutdown: Option<CancellationToken>,
        policy: ConnectionFailurePolicy,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let mut handled_connections = 0_usize;
        let mut handlers = JoinSet::new();
        let mut first_connection_error = None;
        let mut accept_error = None;
        let session_shutdown = shutdown.clone().unwrap_or_default();
        let affinity_record_tasks = TaskTracker::new();
        let connection_handler =
            Arc::new(self.protocol_connection_handler(
                session_shutdown.clone(),
                affinity_record_tasks.clone(),
            ));
        while handled_connections < max_connections {
            let stream = if let Some(shutdown) = shutdown.as_ref() {
                loop {
                    tokio::select! {
                        () = shutdown.cancelled() => break None,
                        joined = handlers.join_next(), if !handlers.is_empty() => {
                            if self.record_owned_connection_result(&mut first_connection_error,joined,policy) {
                                session_shutdown.cancel();
                                break None;
                            }
                        }
                        accepted = self.server.listener.accept() => {
                            match accepted {
                                Ok((stream, _peer_addr)) => break Some(stream),
                                Err(error) => {
                                    accept_error = Some(LoopbackRouterRuntimeError::Accept(error));
                                    break None;
                                }
                            }
                        }
                    }
                }
            } else {
                loop {
                    tokio::select! {
                        joined = handlers.join_next(), if !handlers.is_empty() => {
                            if self.record_owned_connection_result(&mut first_connection_error,joined,policy) {
                                session_shutdown.cancel();
                                break None;
                            }
                        }
                        accepted = self.server.listener.accept() => {
                            match accepted {
                                Ok((stream, _peer_addr)) => break Some(stream),
                                Err(error) => {
                                    accept_error = Some(LoopbackRouterRuntimeError::Accept(error));
                                    break None;
                                }
                            }
                        }
                    }
                }
            };
            let Some(stream) = stream else {
                break;
            };
            let handler_context = Arc::clone(&connection_handler);
            handlers.spawn(
                async move { handler_context.handle_hyper_connection(stream).await }
                    .with_subscriber(self.caller_dispatcher.clone()),
            );
            handled_connections += 1;
            self.enqueue_runtime_maintenance_hints(
                self.fixed_now_unix_seconds
                    .unwrap_or_else(|| current_unix_seconds().unwrap_or(0)),
            );
        }

        self.finish_serving(LoopbackServingCleanupContext {
            handled_connections,
            handlers,
            first_connection_error,
            accept_error,
            session_shutdown,
            affinity_record_tasks,
            connection_failure_policy: policy,
            caller_shutdown_requested: matches!(shutdown.as_ref(), Some(shutdown) if shutdown.is_cancelled()),
        })
        .await
    }

    pub(super) fn record_owned_connection_result(
        &self,
        first_error: &mut Option<LoopbackRouterRuntimeError>,
        joined: Option<Result<Result<(), LoopbackRouterRuntimeError>, JoinError>>,
        policy: ConnectionFailurePolicy,
    ) -> bool {
        match policy {
            ConnectionFailurePolicy::StopServing => {
                store_optional_connection_join_error(first_error, joined)
            }
            ConnectionFailurePolicy::ReportAndContinue => {
                if let Some(joined) = joined
                    && let Err(error) = handle_connection_join_result(joined)
                {
                    self.connection_error_reporter
                        .report_connection_error(&loopback_connection_diagnostic(&error).render());
                }
                false
            }
        }
    }

    pub(super) fn protocol_connection_handler(
        &self,
        session_shutdown: CancellationToken,
        affinity_record_tasks: TaskTracker,
    ) -> LoopbackProtocolConnectionHandler {
        LoopbackProtocolConnectionHandler {
            credential_state_store: self.credential_state_store.clone(),
            provider_error_state_store: self.provider_error_state_store.clone(),
            selection_state_store: self.selection_state_store.clone(),
            credential_store_availability: self.credential_store_availability,
            credential_factory: self.credential_factory.clone(),
            affinity_secret_provider: self.affinity_secret_provider.clone(),
            affinity_owner_recorder: Arc::clone(&self.affinity_owner_recorder),
            affinity_record_tasks,
            auth_gate: self.auth_gate.clone(),
            claude_edge_auth_gate: self.claude_edge_auth_gate.clone(),
            claude_edge_runtime_config: self.claude_edge_runtime_config.clone(),
            local_model_authentication_required: self.local_model_authentication_required,
            upstream: self.upstream.clone(),
            upstream_endpoint: self.upstream_endpoint.clone(),
            websocket_revocations: self.websocket_revocations.clone(),
            audit_sink: self.audit_sink.clone(),
            weighted_selectors: Arc::clone(&self.weighted_selectors),
            account_holds: Arc::clone(&self.account_holds),
            active_reservations: Arc::clone(&self.active_reservations),
            selection_reservation_lock: Arc::clone(&self.selection_reservation_lock),
            session_affinity_cache: Arc::clone(&self.session_affinity_cache),
            claude_five_hour_reserve_percent: self.claude_five_hour_reserve_percent,
            runtime_exhaustions: Arc::clone(&self.runtime_exhaustions),
            route_band_queue_health: Arc::clone(&self.route_band_queue_health),
            db_write_actor: self.db_write_actor.clone(),
            fixed_now_unix_seconds: self.fixed_now_unix_seconds,
            session_shutdown,
        }
    }
}
