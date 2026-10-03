use super::provider_signals::selection_close_reason_from_http_error;
use super::*;

const WEBSOCKET_REQUEST_LOCAL_CREDENTIAL_ATTEMPT_LIMIT: usize = 16;
fn credential_failure_or_websocket_selection_close_reason(
    error: HttpProxyError,
    attempted_accounts: &[AccountId],
) -> WebSocketCloseReason {
    if !attempted_accounts.is_empty()
        && matches!(
            error,
            HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
            }
        )
    {
        return WebSocketCloseReason::ProviderCredential;
    }

    selection_close_reason_from_http_error(error)
}

impl<'a, S, C> AuthenticatedWebSocketRouter<'a, S, C>
where
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    /// Creates an authenticated WebSocket router.
    #[must_use]
    pub const fn new(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
    ) -> Self {
        Self {
            auth_gate,
            selector,
            credential_resolver,
            protocol_router,
            audit_sink: None,
            affinity_secret_provider: None,
        }
    }

    /// Adds a private audit sink.
    #[must_use]
    pub const fn with_audit_sink(mut self, audit_sink: &'a AuditFileSink) -> Self {
        self.audit_sink = Some(audit_sink);
        self
    }

    /// Adds the router-owned affinity secret provider.
    #[must_use]
    pub const fn with_affinity_secret_provider(
        mut self,
        affinity_secret_provider: &'a dyn HttpAffinitySecretProvider,
    ) -> Self {
        self.affinity_secret_provider = Some(affinity_secret_provider);
        self
    }

    fn emit_audit_event(&self, event: AuditEvent) {
        if let Some(audit_sink) = self.audit_sink {
            append_audit_event_with_reporter(audit_sink, &event, &StderrAuditFailureReporter);
        }
    }

    /// Routes one authenticated WebSocket first frame.
    pub fn route_first_frame(
        &self,
        handshake: WebSocketHandshakeRequest,
        first_frame: WebSocketFrame,
    ) -> Result<WebSocketFirstFrameDecision, WebSocketCloseReason> {
        let presented_token = match extract_presented_local_token_from_request(
            handshake.header_value("x-codex-router-token"),
            handshake.header_value("authorization"),
            handshake.header_value("cookie"),
            "",
            &[],
            false,
        ) {
            Ok(presented_token) => presented_token,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::WebSocket,
                    AuditRouteKind::ResponsesWebSocket,
                    reason,
                ));
                return Err(WebSocketCloseReason::LocalAuth { reason });
            }
        };
        let token_generation = match self.auth_gate.authorize(presented_token) {
            Ok(generation) => generation,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::WebSocket,
                    AuditRouteKind::ResponsesWebSocket,
                    reason,
                ));
                return Err(WebSocketCloseReason::LocalAuth { reason });
            }
        };
        self.protocol_router
            .ensure_first_frame_allowed(&first_frame)
            .inspect_err(|_reason| {
                self.emit_audit_event(websocket_first_frame_rejection_audit_event(None));
            })?;
        let mut selection_request = HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_websocket_upgrade(true)
            .with_body(first_frame.payload().to_vec());
        if let Some(session_id) = handshake.header_value("session-id") {
            selection_request =
                selection_request.with_header(Header::new("session-id", session_id));
        }
        let affinity_secret = self.load_affinity_secret().map_err(|_reason| {
            self.emit_audit_event(websocket_selection_rejection_audit_event());
            WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            }
        })?;
        let mut attempted_accounts = Vec::new();
        loop {
            let selected = match self.selector.select_upstream_account(
                &selection_request,
                token_generation,
                Some(&affinity_secret),
            ) {
                Ok(selected) => selected,
                Err(error) => {
                    self.emit_audit_event(websocket_selection_rejection_audit_event());
                    return Err(credential_failure_or_websocket_selection_close_reason(
                        error,
                        &attempted_accounts,
                    ));
                }
            };
            if attempted_accounts
                .iter()
                .any(|account_id| account_id == selected.account_id())
            {
                return Err(WebSocketCloseReason::ProviderCredential);
            }
            let account_hash = redacted_account_hash(selected.account_id());
            let resolved = match self
                .credential_resolver
                .resolve_provider_credentials(selected.account_id(), RESPONSES_WEBSOCKET.provider)
            {
                Ok(resolved) => resolved,
                Err(_reason) => {
                    self.emit_audit_event(websocket_credential_rejection_audit_event(
                        account_hash.clone(),
                    ));
                    if selected.selection_reason() == "previous_response_affinity"
                        || attempted_accounts.len() + 1
                            >= WEBSOCKET_REQUEST_LOCAL_CREDENTIAL_ATTEMPT_LIMIT
                    {
                        return Err(WebSocketCloseReason::ProviderCredential);
                    }
                    tracing::warn!(target: "codex_router_proxy::websocket",
                        { account.hash = account_hash.as_str() },
                        "codex_router.websocket_credential_attempt_failed_retrying_next_account"
                    );
                    attempted_accounts.push(selected.account_id().clone());
                    selection_request =
                        selection_request.with_excluded_account(selected.account_id().clone());
                    continue;
                }
            };

            let decision = self
                .protocol_router
                .route_first_frame(
                    handshake,
                    first_frame,
                    resolved.access_token().clone(),
                    resolved.chatgpt_account_id(),
                )
                .inspect_err(|_reason| {
                    self.emit_audit_event(websocket_first_frame_rejection_audit_event(Some(
                        account_hash.clone(),
                    )));
                })?;
            self.emit_audit_event(allowed_audit_event(
                TransportKind::WebSocket,
                AuditRouteKind::ResponsesWebSocket,
                account_hash,
            ));

            return Ok(match decision {
                WebSocketFirstFrameDecision::OpenUpstream {
                    headers,
                    first_frame,
                    ..
                } => WebSocketFirstFrameDecision::OpenUpstream {
                    token_generation,
                    headers,
                    first_frame,
                    affinity_owner_context: Some(
                        WebSocketAffinityOwnerContext::new(
                            affinity_secret,
                            selected.account_id().clone(),
                            resolved.credential_generation(),
                        )
                        .with_credit_backed_at_selection(selected.credit_backed_at_selection())
                        .with_active_reservation_guard(selected.active_reservation_guard().cloned())
                        .with_session_affinity_activity_handle(
                            selected.session_affinity_activity_handle().cloned(),
                        ),
                    ),
                },
            });
        }
    }

    fn load_affinity_secret(&self) -> Result<RouterAffinityHashSecret, WebSocketCloseReason> {
        let provider = self
            .affinity_secret_provider
            .ok_or(WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            })?;
        provider.load_or_create_affinity_secret().map_err(|_error| {
            WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            }
        })
    }
}

impl<'a, S, C> AsyncAuthenticatedWebSocketRouter<'a, S, C>
where
    S: AsyncAccountDecisionSelector,
    C: AsyncProviderCredentialResolver,
{
    /// Creates an async authenticated WebSocket router.
    #[must_use]
    pub const fn new(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
    ) -> Self {
        Self {
            auth_gate,
            selector,
            credential_resolver,
            protocol_router,
            audit_sink: None,
            affinity_secret_provider: None,
        }
    }

    /// Adds a private audit sink.
    #[must_use]
    pub const fn with_audit_sink(mut self, audit_sink: &'a AuditFileSink) -> Self {
        self.audit_sink = Some(audit_sink);
        self
    }

    /// Adds the router-owned affinity secret provider.
    #[must_use]
    pub const fn with_affinity_secret_provider(
        mut self,
        affinity_secret_provider: &'a dyn HttpAffinitySecretProvider,
    ) -> Self {
        self.affinity_secret_provider = Some(affinity_secret_provider);
        self
    }

    fn emit_audit_event(&self, event: AuditEvent) {
        if let Some(audit_sink) = self.audit_sink {
            append_audit_event_with_reporter(audit_sink, &event, &StderrAuditFailureReporter);
        }
    }

    /// Routes one authenticated WebSocket first frame without blocking on
    /// selector or credential resolution.
    pub async fn route_first_frame(
        &self,
        handshake: WebSocketHandshakeRequest,
        first_frame: WebSocketFrame,
    ) -> Result<WebSocketFirstFrameDecision, WebSocketCloseReason> {
        let presented_token = match extract_presented_local_token_from_request(
            handshake.header_value("x-codex-router-token"),
            handshake.header_value("authorization"),
            handshake.header_value("cookie"),
            "",
            &[],
            false,
        ) {
            Ok(presented_token) => presented_token,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::WebSocket,
                    AuditRouteKind::ResponsesWebSocket,
                    reason,
                ));
                return Err(WebSocketCloseReason::LocalAuth { reason });
            }
        };
        let token_generation = match self.auth_gate.authorize(presented_token) {
            Ok(generation) => generation,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::WebSocket,
                    AuditRouteKind::ResponsesWebSocket,
                    reason,
                ));
                return Err(WebSocketCloseReason::LocalAuth { reason });
            }
        };
        self.protocol_router
            .ensure_first_frame_allowed(&first_frame)
            .inspect_err(|_reason| {
                self.emit_audit_event(websocket_first_frame_rejection_audit_event(None));
            })?;
        let mut selection_request = HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_websocket_upgrade(true)
            .with_body(first_frame.payload().to_vec());
        if let Some(session_id) = handshake.header_value("session-id") {
            selection_request =
                selection_request.with_header(Header::new("session-id", session_id));
        }
        let affinity_secret = self.load_affinity_secret().map_err(|_reason| {
            self.emit_audit_event(websocket_selection_rejection_audit_event());
            WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            }
        })?;
        let mut attempted_accounts = Vec::new();
        loop {
            let selected = match self
                .selector
                .select_upstream_account(
                    &selection_request,
                    token_generation,
                    Some(&affinity_secret),
                )
                .await
            {
                Ok(selected) => selected,
                Err(error) => {
                    self.emit_audit_event(websocket_selection_rejection_audit_event());
                    return Err(credential_failure_or_websocket_selection_close_reason(
                        error,
                        &attempted_accounts,
                    ));
                }
            };
            if attempted_accounts
                .iter()
                .any(|account_id| account_id == selected.account_id())
            {
                return Err(WebSocketCloseReason::ProviderCredential);
            }
            let account_hash = redacted_account_hash(selected.account_id());
            let resolved = match self
                .credential_resolver
                .resolve_provider_credentials(selected.account_id(), RESPONSES_WEBSOCKET.provider)
                .await
            {
                Ok(resolved) => resolved,
                Err(_reason) => {
                    self.emit_audit_event(websocket_credential_rejection_audit_event(
                        account_hash.clone(),
                    ));
                    if selected.selection_reason() == "previous_response_affinity"
                        || attempted_accounts.len() + 1
                            >= WEBSOCKET_REQUEST_LOCAL_CREDENTIAL_ATTEMPT_LIMIT
                    {
                        return Err(WebSocketCloseReason::ProviderCredential);
                    }
                    tracing::warn!(target: "codex_router_proxy::websocket",
                        { account.hash = account_hash.as_str() },
                        "codex_router.async_websocket_credential_attempt_failed_retrying_next_account"
                    );
                    attempted_accounts.push(selected.account_id().clone());
                    selection_request =
                        selection_request.with_excluded_account(selected.account_id().clone());
                    continue;
                }
            };

            let decision = self
                .protocol_router
                .route_first_frame(
                    handshake,
                    first_frame,
                    resolved.access_token().clone(),
                    resolved.chatgpt_account_id(),
                )
                .inspect_err(|_reason| {
                    self.emit_audit_event(websocket_first_frame_rejection_audit_event(Some(
                        account_hash.clone(),
                    )));
                })?;
            self.emit_audit_event(allowed_audit_event(
                TransportKind::WebSocket,
                AuditRouteKind::ResponsesWebSocket,
                account_hash,
            ));

            return Ok(match decision {
                WebSocketFirstFrameDecision::OpenUpstream {
                    headers,
                    first_frame,
                    ..
                } => WebSocketFirstFrameDecision::OpenUpstream {
                    token_generation,
                    headers,
                    first_frame,
                    affinity_owner_context: Some(
                        WebSocketAffinityOwnerContext::new(
                            affinity_secret,
                            selected.account_id().clone(),
                            resolved.credential_generation(),
                        )
                        .with_credit_backed_at_selection(selected.credit_backed_at_selection())
                        .with_active_reservation_guard(selected.active_reservation_guard().cloned())
                        .with_session_affinity_activity_handle(
                            selected.session_affinity_activity_handle().cloned(),
                        ),
                    ),
                },
            });
        }
    }

    fn load_affinity_secret(&self) -> Result<RouterAffinityHashSecret, WebSocketCloseReason> {
        let provider = self
            .affinity_secret_provider
            .ok_or(WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            })?;
        provider.load_or_create_affinity_secret().map_err(|_error| {
            WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            }
        })
    }
}
fn websocket_selection_rejection_audit_event() -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind: AuditRouteKind::ResponsesWebSocket,
        transport_kind: TransportKind::WebSocket,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "selection_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: None,
        error_class: Some("selection"),
    })
}

fn websocket_first_frame_rejection_audit_event(account_hash: Option<String>) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind: AuditRouteKind::ResponsesWebSocket,
        transport_kind: TransportKind::WebSocket,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "first_frame_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash,
        error_class: Some("websocket_first_frame"),
    })
}

fn websocket_credential_rejection_audit_event(account_hash: String) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind: AuditRouteKind::ResponsesWebSocket,
        transport_kind: TransportKind::WebSocket,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "credential_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: Some(account_hash),
        error_class: Some("provider_credential"),
    })
}
