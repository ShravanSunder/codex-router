use super::duplex_forwarding::{WebSocketForwardingContext, forward_duplex_until_complete};
use super::provider_signals::{CODEX_WEBSOCKET_RECONNECT_SIGNAL, short_quota_wait_signal};
use super::response_metadata::is_response_create;
use super::transport_cleanup::{
    close_websocket_stream_best_effort, is_reset_without_closing_handshake,
};
use super::*;

impl<'a, S, C> AsyncWebSocketTunnel<'a, S, C>
where
    S: AsyncAccountDecisionSelector,
    C: AsyncProviderCredentialResolver,
{
    /// Creates an async WebSocket tunnel.
    #[must_use]
    pub fn new(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
    ) -> Self {
        Self {
            router: AsyncAuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            ),
            revocations: WebSocketRevocationRegistry::new(),
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            provider_error_observer: None,
            account_admission_assessor: None,
            session_shutdown: CancellationToken::new(),
            local_peer_addr: None,
        }
    }

    /// Creates an async WebSocket tunnel with a private audit sink.
    #[must_use]
    pub fn new_with_audit_sink(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
        audit_sink: &'a AuditFileSink,
    ) -> Self {
        Self {
            router: AsyncAuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            )
            .with_audit_sink(audit_sink),
            revocations: WebSocketRevocationRegistry::new(),
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            provider_error_observer: None,
            account_admission_assessor: None,
            session_shutdown: CancellationToken::new(),
            local_peer_addr: None,
        }
    }

    /// Adds shared revocation tracking for token rotation.
    #[must_use]
    pub fn with_revocation_registry(mut self, revocations: WebSocketRevocationRegistry) -> Self {
        self.revocations = revocations;
        self
    }

    /// Adds process/runtime shutdown cancellation for active upgraded sessions.
    #[must_use]
    pub fn with_session_shutdown(mut self, session_shutdown: CancellationToken) -> Self {
        self.session_shutdown = session_shutdown;
        self
    }

    /// Adds the router-owned affinity secret provider.
    #[must_use]
    pub fn with_affinity_secret_provider(
        mut self,
        affinity_secret_provider: &'a dyn HttpAffinitySecretProvider,
    ) -> Self {
        self.router = self
            .router
            .with_affinity_secret_provider(affinity_secret_provider);
        self
    }

    /// Adds the previous-response owner recorder.
    #[must_use]
    pub fn with_affinity_owner_recorder(
        mut self,
        affinity_owner_recorder: Arc<dyn HttpAffinityOwnerRecorder>,
    ) -> Self {
        self.affinity_owner_recorder = Some(affinity_owner_recorder);
        self
    }

    /// Adds the async previous-response owner recorder for production Tokio runtime callers.
    #[must_use]
    pub fn with_async_affinity_owner_recorder(
        mut self,
        affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    ) -> Self {
        self.async_affinity_owner_recorder = Some(affinity_owner_recorder);
        self
    }

    /// Adds the task tracker used to drain non-blocking affinity-owner writes on shutdown.
    #[must_use]
    pub fn with_affinity_owner_task_tracker(mut self, affinity_record_tasks: TaskTracker) -> Self {
        self.affinity_record_tasks = affinity_record_tasks;
        self
    }

    /// Adds async provider error observer for quota accounting.
    #[must_use]
    pub fn with_provider_error_observer(
        mut self,
        provider_error_observer: Arc<dyn AsyncProviderErrorObserver>,
    ) -> Self {
        self.provider_error_observer = Some(provider_error_observer);
        self
    }

    /// Adds the live read-only peer assessor for graceful weekly-floor switching.
    #[must_use]
    pub(crate) fn with_account_admission_assessor(
        mut self,
        peer_assessor: Arc<dyn LiveAccountAdmissionAssessor>,
    ) -> Self {
        self.account_admission_assessor = Some(peer_assessor);
        self
    }

    /// Adds the local peer socket address observed by the Hyper accept path.
    #[must_use]
    pub fn with_local_peer_addr(mut self, local_peer_addr: Option<SocketAddr>) -> Self {
        self.local_peer_addr = local_peer_addr;
        self
    }

    /// Handles one already-upgraded local WebSocket stream.
    pub async fn handle_upgraded_connection<LocalStream>(
        &self,
        mut local_websocket: WebSocketStream<LocalStream>,
        handshake: WebSocketHandshakeRequest,
        upstream_url: &str,
    ) -> Result<(), WebSocketTunnelError>
    where
        LocalStream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let Some(first_message) =
            next_data_message_before_upstream(&mut local_websocket, &self.session_shutdown).await?
        else {
            return Ok(());
        };
        let original_first_frame = frame_from_message(first_message);
        let mut attempted_accounts = Vec::new();
        let (
            mut upstream_websocket,
            session_registration,
            affinity_owner_context,
            first_frame,
            revocation,
        ) = loop {
            let decision = tokio::select! {
                () = self.session_shutdown.cancelled() => {
                    close_websocket_stream_best_effort(&mut local_websocket).await?;
                    return Ok(());
                }
                decision = self.router.route_first_frame_with_attempted_accounts(
                    handshake.clone(), original_first_frame.clone(), &mut attempted_accounts
                ) => {
                    match decision {
                        Ok(decision) => decision,
                        Err(close_reason) => {
                            if handle_pre_upstream_close_reason(&mut local_websocket, &close_reason).await? {
                                return Ok(());
                            }
                            return Err(WebSocketTunnelError::CloseReason(close_reason));
                        }
                    }
                }
            };
            let WebSocketFirstFrameDecision::OpenUpstream {
                token_generation,
                headers,
                first_frame,
                affinity_owner_context,
            } = decision;
            let selected_account_id = match affinity_owner_context.as_ref() {
                Some(context) => context.account_id.clone(),
                None => {
                    return Err(WebSocketTunnelError::TaskJoin(
                        "selected websocket account context was missing".to_owned(),
                    ));
                }
            };
            let session_registration = self.revocations.register_cancellation_with_peer_addr(
                token_generation,
                selected_account_id,
                self.local_peer_addr,
            );
            self.revocations.set_capacity_retry_thread_id(
                session_registration.session_id,
                capacity_retry_thread_id(&headers),
            );
            let revocation = session_registration.cancellation().clone();

            let mut upstream_request = upstream_url.into_client_request()?;
            apply_upstream_headers(upstream_request.headers_mut(), &headers)?;
            let connection = tokio::select! {
                biased;
                () = session_registration.quota_floor_reconnect.cancelled() => {
                    self.revocations.note_quota_reconnect_signal();
                    local_websocket
                        .send(Message::text(CODEX_WEBSOCKET_RECONNECT_SIGNAL))
                        .await?;
                    close_websocket_stream_best_effort(&mut local_websocket).await?;
                    return Ok(());
                }
                () = self.session_shutdown.cancelled() => {
                    close_websocket_stream_best_effort(&mut local_websocket).await?;
                    return Ok(());
                }
                () = revocation.cancelled() => {
                    close_websocket_stream_best_effort(&mut local_websocket).await?;
                    return Ok(());
                }
                connection = connect_async_with_config(upstream_request, Some(router_websocket_config()), false) => connection,
            };
            match connection {
                Ok((upstream_websocket, _response)) => {
                    break (
                        upstream_websocket,
                        session_registration,
                        affinity_owner_context,
                        first_frame,
                        revocation,
                    );
                }
                Err(error) if matches!(&error, tungstenite::Error::Http(response) if response.status().as_u16() == 401) =>
                {
                    // The shared vector excludes this failed handshake candidate;
                    // release its connection/load ownership before choosing another.
                    drop(session_registration);
                    drop(affinity_owner_context);
                }
                Err(error) => return Err(WebSocketTunnelError::Transport(error)),
            }
        };
        let upstream_first_message = message_from_frame(first_frame)?;
        let initial_turn_active = is_response_create(&upstream_first_message);
        tokio::select! {
            biased;
            () = session_registration.quota_floor_reconnect.cancelled() => {
                self.revocations.note_quota_reconnect_signal();
                local_websocket
                    .send(Message::text(CODEX_WEBSOCKET_RECONNECT_SIGNAL))
                    .await?;
                close_websocket_stream_best_effort(&mut local_websocket).await?;
                close_websocket_stream_best_effort(&mut upstream_websocket).await?;
                return Ok(());
            }
            () = self.session_shutdown.cancelled() => {
                close_websocket_stream_best_effort(&mut local_websocket).await?;
                close_websocket_stream_best_effort(&mut upstream_websocket).await?;
                return Ok(());
            }
            () = revocation.cancelled() => {
                close_websocket_stream_best_effort(&mut local_websocket).await?;
                close_websocket_stream_best_effort(&mut upstream_websocket).await?;
                return Ok(());
            }
            result = upstream_websocket.send(upstream_first_message) => {
                result?;
            }
        }

        forward_duplex_until_complete(
            local_websocket,
            upstream_websocket,
            WebSocketForwardingContext {
                session_registration,
                affinity_owner_recorder: self.affinity_owner_recorder.clone(),
                async_affinity_owner_recorder: self.async_affinity_owner_recorder.clone(),
                affinity_record_tasks: self.affinity_record_tasks.clone(),
                affinity_owner_context: affinity_owner_context.as_ref(),
                provider_error_observer: self.provider_error_observer.clone(),
                account_admission_assessor: self.account_admission_assessor.clone(),
                initial_turn_active,
                revocation: &revocation,
                session_shutdown: &self.session_shutdown,
            },
        )
        .await
    }
}
pub(super) fn capacity_retry_thread_id(headers: &HeaderCollection) -> Option<String> {
    let values = headers.values("thread-id");
    let [thread_id] = values.as_slice() else {
        return None;
    };
    if thread_id.is_empty() || thread_id.len() > MAX_THREAD_ID_BYTES {
        return None;
    }
    Some((*thread_id).to_owned())
}

async fn handle_pre_upstream_close_reason<LocalStream>(
    local_websocket: &mut WebSocketStream<LocalStream>,
    close_reason: &WebSocketCloseReason,
) -> Result<bool, WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin,
{
    let Some(router_signal) = pre_upstream_router_signal(close_reason) else {
        return Ok(false);
    };
    local_websocket.send(Message::text(router_signal)).await?;
    close_websocket_stream_best_effort(local_websocket).await?;
    Ok(true)
}

fn pre_upstream_router_signal(close_reason: &WebSocketCloseReason) -> Option<String> {
    match close_reason {
        WebSocketCloseReason::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        } => Some(ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL.to_owned()),
        WebSocketCloseReason::Selection {
            reason:
                QuotaAwareAccountSelectorError::ShortQuotaExhausted {
                    retry_after_seconds,
                },
        } => Some(short_quota_wait_signal(*retry_after_seconds)),
        WebSocketCloseReason::Selection {
            reason:
                QuotaAwareAccountSelectorError::StateUnavailable
                | QuotaAwareAccountSelectorError::SelectorStateUnavailable,
        } => Some(ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL.to_owned()),
        _ => None,
    }
}

async fn next_data_message_before_upstream<LocalStream>(
    local_websocket: &mut WebSocketStream<LocalStream>,
    session_shutdown: &CancellationToken,
) -> Result<Option<Message>, WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let message = tokio::select! {
            () = session_shutdown.cancelled() => {
                close_websocket_stream_best_effort(local_websocket).await?;
                return Ok(None);
            }
            message = local_websocket.next() => message,
        };

        match message {
            Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                return Ok(Some(message));
            }
            Some(Ok(Message::Ping(payload))) => {
                local_websocket.send(Message::Pong(payload)).await?;
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_close_frame))) => return Ok(None),
            Some(Ok(Message::Frame(_))) => {}
            Some(Err(error)) if is_reset_without_closing_handshake(&error) => return Ok(None),
            Some(Err(error)) => return Err(WebSocketTunnelError::Transport(error)),
            None => return Ok(None),
        }
    }
}
pub(super) fn frame_from_message(message: Message) -> WebSocketFrame {
    match message {
        Message::Text(value) => WebSocketFrame::Text(value.as_str().as_bytes().to_vec()),
        Message::Binary(value) => WebSocketFrame::Binary(value.to_vec()),
        _other => WebSocketFrame::Binary(Vec::new()),
    }
}

pub(super) fn apply_upstream_headers(
    target: &mut tungstenite::http::HeaderMap,
    headers: &HeaderCollection,
) -> Result<(), WebSocketTunnelError> {
    for header in headers.as_slice() {
        let name = HeaderName::from_str(header.name()).map_err(|_| {
            WebSocketTunnelError::InvalidUpstreamHeader {
                name: header.name().to_owned(),
            }
        })?;
        let value = HeaderValue::from_str(header.value()).map_err(|_| {
            WebSocketTunnelError::InvalidUpstreamHeader {
                name: header.name().to_owned(),
            }
        })?;
        target.insert(name, value);
    }

    Ok(())
}

pub(super) fn message_from_frame(frame: WebSocketFrame) -> Result<Message, WebSocketTunnelError> {
    match frame {
        WebSocketFrame::Text(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| WebSocketTunnelError::InvalidText)?;
            Ok(Message::text(text))
        }
        WebSocketFrame::Binary(bytes) => Ok(Message::binary(bytes)),
    }
}
