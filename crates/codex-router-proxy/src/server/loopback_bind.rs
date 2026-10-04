/// Address validated for the v1 loopback-only proxy server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopbackBindAddress {
    host: IpAddr,
    port: u16,
}

impl LoopbackBindAddress {
    /// Creates a bind address after rejecting non-loopback hosts.
    pub fn new(host: impl AsRef<str>, port: u16) -> Result<Self, ServerBindError> {
        let host_text = host.as_ref();
        let host_address = parse_loopback_candidate(host_text)?;

        if !host_address.is_loopback() {
            return Err(ServerBindError::NonLoopback {
                host: host_text.to_owned(),
            });
        }

        Ok(Self {
            host: host_address,
            port,
        })
    }

    /// Returns the socket address used for binding.
    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

fn parse_loopback_candidate(host: &str) -> Result<IpAddr, ServerBindError> {
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    host.parse::<IpAddr>()
        .map_err(|source| ServerBindError::InvalidHost {
            host: host.to_owned(),
            source,
        })
}

/// Bound loopback listener kept alive by the router runtime.
#[cfg(test)]
#[derive(Debug)]
pub struct LoopbackServerRuntime {
    listener: TcpListener,
    local_addr: SocketAddr,
}

#[cfg(test)]
impl LoopbackServerRuntime {
    /// Binds a TCP listener to a validated loopback address.
    pub fn bind(address: LoopbackBindAddress) -> Result<Self, ServerBindError> {
        let socket_addr = address.socket_addr();
        let listener = TcpListener::bind(socket_addr).map_err(|source| ServerBindError::Bind {
            address: socket_addr,
            source,
        })?;
        let local_addr = listener
            .local_addr()
            .map_err(|source| ServerBindError::Bind {
                address: socket_addr,
                source,
            })?;

        Ok(Self {
            listener,
            local_addr,
        })
    }

    /// Returns the actual local address, including kernel-assigned port.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Returns the bound listener.
    #[must_use]
    pub fn listener(&self) -> &TcpListener {
        &self.listener
    }
}

/// Tokio-owned loopback listener substrate for the async release runtime.
///
/// This is intentionally only the T1 listener/task shell. HTTP/SSE routing,
/// WebSocket upgrade handling, and pump behavior are cut over in later slices.
#[derive(Debug)]
pub struct AsyncLoopbackServerRuntime {
    listener: TokioTcpListener,
    local_addr: SocketAddr,
}

impl AsyncLoopbackServerRuntime {
    /// Binds a Tokio TCP listener to a validated loopback address.
    pub async fn bind(address: LoopbackBindAddress) -> Result<Self, ServerBindError> {
        let socket_addr = address.socket_addr();
        let listener = TokioTcpListener::bind(socket_addr)
            .await
            .map_err(|source| ServerBindError::Bind {
                address: socket_addr,
                source,
            })?;
        let local_addr = listener
            .local_addr()
            .map_err(|source| ServerBindError::Bind {
                address: socket_addr,
                source,
            })?;

        Ok(Self {
            listener,
            local_addr,
        })
    }

    /// Returns the actual local address, including kernel-assigned port.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Runs the async accept shell until cancellation.
    ///
    /// T1 accepts and immediately drops streams because the Hyper service,
    /// HTTP/SSE body forwarding, and WebSocket pumps are later plan slices.
    pub async fn serve_until_cancelled(
        self,
        shutdown: CancellationToken,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let mut handled_connections = 0_usize;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return Ok(handled_connections),
                accepted = self.listener.accept() => {
                    let (_stream, _peer_addr) = accepted
                        .map_err(LoopbackRouterRuntimeError::Accept)?;
                    handled_connections += 1;
                }
            }
        }
    }
}

/// First routing decision made by the future Hyper service switchpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HyperProtocolDispatch {
    /// Ordinary HTTP/SSE request path.
    Http,
    /// WebSocket upgrade request path.
    WebSocketUpgrade,
}

/// Shared Hyper request switchpoint for HTTP/SSE and WebSocket paths.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HyperProtocolSwitchpoint;

impl HyperProtocolSwitchpoint {
    /// Classifies a Hyper request head without consuming or buffering the body.
    #[must_use]
    pub fn classify(
        _method: &HttpMethod,
        _uri: &Uri,
        headers: &HeaderMap,
    ) -> HyperProtocolDispatch {
        if is_websocket_upgrade(headers) {
            HyperProtocolDispatch::WebSocketUpgrade
        } else {
            HyperProtocolDispatch::Http
        }
    }
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    let has_upgrade_header = headers
        .get(http::header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let has_connection_upgrade = headers
        .get(http::header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
        });

    has_upgrade_header && has_connection_upgrade
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClaudeEdgeRuntimeConfig {
    local_token: LocalRouterTokenRecord,
    quota_refresh_interval: Duration,
}
