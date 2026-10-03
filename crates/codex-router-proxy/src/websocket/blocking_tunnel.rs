use super::async_tunnel::{apply_upstream_headers, frame_from_message, message_from_frame};
use super::response_metadata::{is_response_completed, record_websocket_affinity_owner};
use super::transport_cleanup::router_websocket_config;
use super::*;

#[cfg(test)]
impl<'a, S, C> BlockingWebSocketTunnel<'a, S, C>
where
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    /// Creates a blocking WebSocket tunnel.
    #[must_use]
    pub fn new(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
    ) -> Self {
        Self {
            router: AuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            ),
            revocations: WebSocketRevocationRegistry::new(),
            affinity_owner_recorder: None,
        }
    }

    /// Creates a blocking WebSocket tunnel with shared revocation tracking.
    #[must_use]
    pub fn new_with_revocation_registry(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
        revocations: WebSocketRevocationRegistry,
    ) -> Self {
        Self {
            router: AuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            ),
            revocations,
            affinity_owner_recorder: None,
        }
    }

    /// Creates a blocking WebSocket tunnel with revocation tracking and a private audit sink.
    #[must_use]
    pub fn new_with_revocation_registry_and_audit_sink(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
        revocations: WebSocketRevocationRegistry,
        audit_sink: &'a AuditFileSink,
    ) -> Self {
        Self {
            router: AuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            )
            .with_audit_sink(audit_sink),
            revocations,
            affinity_owner_recorder: None,
        }
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
        affinity_owner_recorder: &'a dyn HttpAffinityOwnerRecorder,
    ) -> Self {
        self.affinity_owner_recorder = Some(affinity_owner_recorder);
        self
    }

    /// Handles one local WebSocket connection and forwards a bounded upstream transcript.
    pub fn handle_connection(
        &self,
        local_stream: TcpStream,
        upstream_url: &str,
        max_upstream_messages: usize,
    ) -> Result<(), WebSocketTunnelError> {
        let captured_handshake = Arc::new(Mutex::new(None));
        let handshake_for_callback = Arc::clone(&captured_handshake);
        let mut local_websocket =
            accept_local_websocket(local_stream, move |request: &Request| {
                let handshake = handshake_from_request(request);
                match handshake_for_callback.lock() {
                    Ok(mut captured) => {
                        *captured = Some(handshake);
                    }
                    Err(_error) => {}
                }
            })?;
        let handshake = take_captured_handshake(&captured_handshake)?;
        let first_message = match local_websocket.read() {
            Ok(message) => message,
            Err(error) => return Err(WebSocketTunnelError::Transport(error)),
        };
        let first_frame = frame_from_message(first_message);
        let decision = self
            .router
            .route_first_frame(handshake, first_frame)
            .map_err(WebSocketTunnelError::CloseReason)?;
        let WebSocketFirstFrameDecision::OpenUpstream {
            token_generation,
            headers,
            first_frame,
            affinity_owner_context,
        } = decision;
        self.revocations
            .register(token_generation, local_websocket.get_ref())?;

        let mut upstream_request = upstream_url.into_client_request()?;
        apply_upstream_headers(upstream_request.headers_mut(), &headers)?;
        let (mut upstream_websocket, _response) =
            connect_with_config(upstream_request, Some(router_websocket_config()), 0)?;
        upstream_websocket.send(message_from_frame(first_frame)?)?;
        forward_upstream_response(
            &mut upstream_websocket,
            &mut local_websocket,
            max_upstream_messages,
            self.affinity_owner_recorder,
            affinity_owner_context.as_ref(),
        )?;
        local_websocket
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|error| WebSocketTunnelError::Transport(tungstenite::Error::Io(error)))?;

        loop {
            let local_message = match local_websocket.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(error))
                    if error.kind() == ErrorKind::WouldBlock
                        || error.kind() == ErrorKind::TimedOut =>
                {
                    local_websocket.close(None)?;
                    upstream_websocket.close(None)?;
                    return Ok(());
                }
                Err(error) => return Err(WebSocketTunnelError::Transport(error)),
            };
            let is_close = matches!(local_message, Message::Close(_));
            upstream_websocket.send(local_message)?;
            if is_close {
                return Ok(());
            }
            forward_upstream_response(
                &mut upstream_websocket,
                &mut local_websocket,
                max_upstream_messages,
                self.affinity_owner_recorder,
                affinity_owner_context.as_ref(),
            )?;
        }
    }
}
#[cfg(test)]
fn forward_upstream_response(
    upstream_websocket: &mut WebSocket<impl std::io::Read + std::io::Write>,
    local_websocket: &mut WebSocket<impl std::io::Read + std::io::Write>,
    max_upstream_messages: usize,
    affinity_owner_recorder: Option<&dyn HttpAffinityOwnerRecorder>,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) -> Result<(), WebSocketTunnelError> {
    for _ in 0..max_upstream_messages {
        let upstream_message = upstream_websocket.read()?;
        let is_close = matches!(upstream_message, Message::Close(_));
        let is_completed = is_response_completed(&upstream_message);
        record_websocket_affinity_owner(
            &upstream_message,
            affinity_owner_recorder,
            affinity_owner_context,
        );
        local_websocket.send(upstream_message)?;
        if is_close || is_completed {
            return Ok(());
        }
    }
    local_websocket.close(None)?;
    upstream_websocket.close(None)?;

    Ok(())
}
#[allow(clippy::result_large_err)]
#[cfg(test)]
fn accept_local_websocket<F>(
    local_stream: TcpStream,
    on_request: F,
) -> Result<WebSocket<TcpStream>, WebSocketTunnelError>
where
    F: FnOnce(&Request),
{
    accept_hdr_with_config(
        local_stream,
        move |request: &Request, response: Response| {
            on_request(request);
            Ok(response)
        },
        Some(router_websocket_config()),
    )
    .map_err(|_error| WebSocketTunnelError::Handshake)
}

#[cfg(test)]
fn handshake_from_request(request: &Request) -> WebSocketHandshakeRequest {
    request
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| Header::new(name.as_str(), value))
        })
        .fold(WebSocketHandshakeRequest::new(), |handshake, header| {
            handshake.with_header(header)
        })
}

#[cfg(test)]
fn take_captured_handshake(
    captured_handshake: &Mutex<Option<WebSocketHandshakeRequest>>,
) -> Result<WebSocketHandshakeRequest, WebSocketTunnelError> {
    let mut captured = captured_handshake
        .lock()
        .map_err(|_| WebSocketTunnelError::HandshakeCapture)?;
    captured
        .take()
        .ok_or(WebSocketTunnelError::HandshakeCapture)
}
