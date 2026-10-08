use super::*;

/// Assembled router runtime failure.
#[derive(Debug, thiserror::Error)]
pub enum LoopbackRouterRuntimeError {
    #[error("failed to inspect state database path")]
    StateInspection(#[source] std::io::Error),
    #[error(transparent)]
    SchemaPreparation(#[from] codex_router_state::schema_preparation::StateSchemaPreparationError),
    #[error(transparent)]
    ListenerGrant(#[from] codex_router_descriptor_boundary::BoundaryError),
    #[error("granted TCP listener does not match the loopback address")]
    ListenerAssociation,

    /// Binding the loopback listener failed.
    #[error(transparent)]
    Bind(#[from] ServerBindError),
    /// Accepting a loopback connection failed.
    #[error("failed accepting loopback router connection")]
    Accept(#[source] std::io::Error),
    /// Opening or reading SQLite state failed.
    #[error(transparent)]
    State(#[from] StateStoreError),
    /// Opening runtime credential resources failed.
    #[error(transparent)]
    CredentialResources(#[from] ProxyRuntimeCredentialResourcesOpenError),
    /// Runtime system clock is before Unix epoch.
    #[error("system clock is before Unix epoch")]
    SystemClock(#[source] std::time::SystemTimeError),
    /// Tokio runtime creation failed.
    #[error("failed to create Tokio runtime")]
    TokioRuntime(#[source] std::io::Error),
    /// Hyper connection serving failed.
    #[error("failed serving Hyper loopback connection")]
    HyperConnection(#[source] hyper::Error),
    /// Hyper request body collection failed.
    #[error("failed reading Hyper request body")]
    HyperBody(#[source] hyper::Error),
    /// A real stored actor task failed after terminal completion.
    #[error("proxy {actor:?} actor task failed")]
    ActorJoin {
        actor: LoopbackActorTask,
        #[source]
        source: tokio::task::JoinError,
    },
    /// Hyper connection task failed.
    #[error("Hyper connection task failed")]
    ConnectionJoin(#[source] tokio::task::JoinError),
    /// Serving a loopback connection failed.
    #[cfg(test)]
    #[error(transparent)]
    Connection(#[from] ServerConnectionError),
    /// Serving a WebSocket tunnel failed.
    #[error(transparent)]
    WebSocket(#[from] crate::websocket::WebSocketTunnelError),
}

/// Server bind validation and runtime errors.
#[derive(Debug, thiserror::Error)]
pub enum ServerBindError {
    /// Host was not an IP address.
    #[error("invalid listen host `{host}`")]
    InvalidHost {
        /// Original host text.
        host: String,
        /// Parse failure.
        source: AddrParseError,
    },
    /// Host was valid but not loopback.
    #[error("listen host `{host}` is not loopback")]
    NonLoopback {
        /// Rejected host text.
        host: String,
    },
    /// TCP bind failed for the validated address.
    #[error("failed to bind loopback listener at {address}")]
    Bind {
        /// Address passed to the kernel.
        address: SocketAddr,
        /// I/O failure from bind or local address lookup.
        source: std::io::Error,
    },
}

impl PartialEq for ServerBindError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::InvalidHost { host: left, .. }, Self::InvalidHost { host: right, .. }) => {
                left == right
            }
            (Self::NonLoopback { host: left }, Self::NonLoopback { host: right }) => left == right,
            (Self::Bind { address: left, .. }, Self::Bind { address: right, .. }) => left == right,
            _ => false,
        }
    }
}

/// Adapter for one loopback HTTP/1.x connection.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct LoopbackHttpAdapter;

#[cfg(test)]
impl LoopbackHttpAdapter {
    /// Handles one accepted HTTP connection.
    pub fn handle_connection<H>(
        mut stream: TcpStream,
        handler: &H,
    ) -> Result<(), ServerConnectionError>
    where
        H: HttpRequestHandler,
    {
        let request = read_http_request(&mut stream)?;
        let response = match handler.handle_request(request) {
            Ok(response) => response,
            Err(HttpProxyError::LocalAuth { .. }) => {
                write_http_error_response(&mut stream, 401, "Unauthorized")?;
                return Ok(());
            }
            Err(HttpProxyError::Selection { .. }) => {
                write_http_error_response(&mut stream, 503, "Service Unavailable")?;
                return Ok(());
            }
            Err(HttpProxyError::ProviderCredential { .. }) => {
                write_http_error_response(&mut stream, 502, "Bad Gateway")?;
                return Ok(());
            }
            Err(error) => return Err(ServerConnectionError::Proxy(error)),
        };
        write_http_response(&mut stream, response)?;

        Ok(())
    }

    /// Handles one accepted HTTP connection without buffering response bodies.
    pub fn handle_streaming_connection<H>(
        mut stream: TcpStream,
        handler: &H,
    ) -> Result<(), ServerConnectionError>
    where
        H: StreamingHttpRequestHandler,
    {
        let request = read_http_request(&mut stream)?;
        let response = match handler.handle_streaming_request(request) {
            Ok(response) => response,
            Err(HttpProxyError::LocalAuth { .. }) => {
                write_http_error_response(&mut stream, 401, "Unauthorized")?;
                return Ok(());
            }
            Err(HttpProxyError::Selection { .. }) => {
                write_http_error_response(&mut stream, 503, "Service Unavailable")?;
                return Ok(());
            }
            Err(HttpProxyError::ProviderCredential { .. }) => {
                write_http_error_response(&mut stream, 502, "Bad Gateway")?;
                return Ok(());
            }
            Err(error) => return Err(ServerConnectionError::Proxy(error)),
        };
        write_streaming_http_response(&mut stream, response)?;

        Ok(())
    }
}

/// Bounded loopback HTTP server accept loop.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct LoopbackHttpServer;

#[cfg(test)]
impl LoopbackHttpServer {
    /// Accepts and handles loopback HTTP connections until the bound is reached.
    pub fn serve_connections<H>(
        listener: TcpListener,
        handler: &H,
        max_connections: usize,
    ) -> Result<usize, ServerConnectionError>
    where
        H: HttpRequestHandler,
    {
        let mut handled_connections = 0_usize;
        while handled_connections < max_connections {
            let (stream, _peer_addr) = listener.accept().map_err(ServerConnectionError::Accept)?;
            LoopbackHttpAdapter::handle_connection(stream, handler)?;
            handled_connections += 1;
        }

        Ok(handled_connections)
    }

    /// Accepts and handles loopback HTTP connections without buffering response bodies.
    pub fn serve_streaming_connections<H>(
        listener: TcpListener,
        handler: &H,
        max_connections: usize,
    ) -> Result<usize, ServerConnectionError>
    where
        H: StreamingHttpRequestHandler,
    {
        let mut handled_connections = 0_usize;
        while handled_connections < max_connections {
            let (stream, _peer_addr) = listener.accept().map_err(ServerConnectionError::Accept)?;
            LoopbackHttpAdapter::handle_streaming_connection(stream, handler)?;
            handled_connections += 1;
        }

        Ok(handled_connections)
    }
}

#[cfg(test)]
pub(crate) fn read_http_request(
    stream: &mut TcpStream,
) -> Result<HttpProxyRequest, ServerConnectionError> {
    let mut request_bytes = Vec::new();
    let parsed_head = loop {
        if request_bytes.len() > MAX_HTTP_HEADER_BYTES {
            return Err(ServerConnectionError::HeaderTooLarge);
        }
        if let Some(parsed_head) = parse_http_request_head(&request_bytes)? {
            break parsed_head;
        }

        let mut buffer = [0_u8; 4096];
        let read = stream
            .read(&mut buffer)
            .map_err(ServerConnectionError::Read)?;
        if read == 0 {
            return Err(ServerConnectionError::PartialRequest);
        }
        request_bytes.extend_from_slice(&buffer[..read]);
    };
    let body_end = parsed_head
        .header_length
        .checked_add(parsed_head.content_length)
        .ok_or(ServerConnectionError::BodyTooLarge)?;
    while request_bytes.len() < body_end {
        let mut buffer = [0_u8; 4096];
        let read = stream
            .read(&mut buffer)
            .map_err(ServerConnectionError::Read)?;
        if read == 0 {
            return Err(ServerConnectionError::PartialBody);
        }
        request_bytes.extend_from_slice(&buffer[..read]);
    }

    let body = request_bytes[parsed_head.header_length..body_end].to_vec();
    let mut request = HttpProxyRequest::new(parsed_head.method, parsed_head.path);
    for header in parsed_head.headers {
        request = request.with_header(header);
    }

    Ok(request.with_body(body))
}

#[derive(Debug)]
#[cfg(test)]
pub(super) struct ParsedHttpRequestHead {
    pub(super) method: Method,
    pub(super) path: String,
    pub(super) headers: Vec<Header>,
    pub(super) header_length: usize,
    pub(super) content_length: usize,
}

#[cfg(test)]
pub(super) fn parse_http_request_head(
    request_bytes: &[u8],
) -> Result<Option<ParsedHttpRequestHead>, ServerConnectionError> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed_request = httparse::Request::new(&mut headers);
    let header_length = match parsed_request.parse(request_bytes) {
        Ok(httparse::Status::Complete(header_length)) => header_length,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(source) => return Err(ServerConnectionError::Parse(source)),
    };
    let method = method_from_http(
        parsed_request
            .method
            .ok_or(ServerConnectionError::MissingMethod)?,
    );
    let path = parsed_request
        .path
        .ok_or(ServerConnectionError::MissingPath)?
        .to_owned();
    let content_length = parsed_request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-length"))
        .and_then(|header| std::str::from_utf8(header.value).ok())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let mut request_headers = Vec::new();
    for header in parsed_request.headers.iter() {
        let value = std::str::from_utf8(header.value).map_err(ServerConnectionError::HeaderUtf8)?;
        request_headers.push(Header::new(header.name, value));
    }

    Ok(Some(ParsedHttpRequestHead {
        method,
        path,
        headers: request_headers,
        header_length,
        content_length,
    }))
}

#[cfg(test)]
pub(super) fn method_from_http(method: &str) -> Method {
    match method {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => Method::Other,
    }
}

#[cfg(test)]
pub(super) fn write_http_response(
    stream: &mut TcpStream,
    response: HttpProxyResponse,
) -> Result<(), ServerConnectionError> {
    write!(stream, "HTTP/1.1 {} OK\r\n", response.status())
        .map_err(ServerConnectionError::Write)?;
    for header in response.headers().as_slice() {
        write!(stream, "{}: {}\r\n", header.name(), header.value())
            .map_err(ServerConnectionError::Write)?;
    }
    write!(stream, "Content-Length: {}\r\n\r\n", response.body().len())
        .map_err(ServerConnectionError::Write)?;
    stream
        .write_all(response.body())
        .map_err(ServerConnectionError::Write)
}

#[cfg(test)]
pub(super) fn write_http_error_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
) -> Result<(), ServerConnectionError> {
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .map_err(ServerConnectionError::Write)
}

#[cfg(test)]
pub(super) fn write_streaming_http_response(
    stream: &mut TcpStream,
    mut response: StreamingHttpProxyResponse,
) -> Result<(), ServerConnectionError> {
    write!(stream, "HTTP/1.1 {} OK\r\n", response.status())
        .map_err(ServerConnectionError::Write)?;
    for header in response.headers().as_slice() {
        write!(stream, "{}: {}\r\n", header.name(), header.value())
            .map_err(ServerConnectionError::Write)?;
    }
    stream
        .write_all(b"\r\n")
        .map_err(ServerConnectionError::Write)?;
    stream.flush().map_err(ServerConnectionError::Write)?;
    std::io::copy(response.body_mut(), stream).map_err(ServerConnectionError::Write)?;
    stream.flush().map_err(ServerConnectionError::Write)?;

    Ok(())
}

/// One-connection loopback HTTP adapter failure.
#[cfg(test)]
#[derive(Debug, thiserror::Error)]
pub enum ServerConnectionError {
    /// Accepting a loopback connection failed.
    #[error("failed accepting loopback HTTP connection")]
    Accept(#[source] std::io::Error),
    /// Reading from the accepted stream failed.
    #[error("failed reading HTTP connection")]
    Read(#[source] std::io::Error),
    /// Request bytes were not a complete HTTP request.
    #[error("partial HTTP request")]
    PartialRequest,
    /// Request headers exceeded the local parsing bound.
    #[error("HTTP headers too large")]
    HeaderTooLarge,
    /// Request body was incomplete.
    #[error("partial HTTP body")]
    PartialBody,
    /// Request body size overflowed local indexing.
    #[error("HTTP body too large")]
    BodyTooLarge,
    /// HTTP parser rejected request bytes.
    #[error("failed parsing HTTP request")]
    Parse(#[source] httparse::Error),
    /// Header value was not valid UTF-8.
    #[error("HTTP header value was not valid UTF-8")]
    HeaderUtf8(#[source] std::str::Utf8Error),
    /// Request path was missing.
    #[error("HTTP request path was missing")]
    MissingPath,
    /// Request method was missing.
    #[error("HTTP request method was missing")]
    MissingMethod,
    /// Proxy service rejected or failed the request.
    #[error(transparent)]
    Proxy(#[from] HttpProxyError),
    /// Writing to the accepted stream failed.
    #[error("failed writing HTTP response")]
    Write(#[source] std::io::Error),
}

/// Existing actor handle whose actual completion reported a failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopbackActorTask {
    DatabaseWrites,
    SessionAffinity,
    Maintenance,
}
