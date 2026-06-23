//! Upstream request construction.

use std::io::Cursor;
use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::net::TcpStream;

use codex_router_core::redaction::SecretString;
use thiserror::Error;

use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::headers::sanitize_headers_for_upstream;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyResponse;
use crate::http_sse::StreamingHttpProxyResponse;
use crate::http_sse::StreamingUpstreamHttpTransport;
use crate::http_sse::UpstreamHttpRequest;
use crate::http_sse::UpstreamHttpTransport;
use crate::routes::RouteKind;

/// Upstream provider endpoint used to build request URLs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamEndpoint {
    base_url: String,
}

impl UpstreamEndpoint {
    /// Creates an upstream endpoint from a provider base URL.
    pub fn new(base_url: impl Into<String>) -> Result<Self, UpstreamEndpointError> {
        let base_url = base_url.into();
        let trimmed = base_url.trim_end_matches('/').to_owned();
        if trimmed.is_empty() {
            return Err(UpstreamEndpointError::Empty);
        }
        if !trimmed.starts_with("https://") && !trimmed.starts_with("http://") {
            return Err(UpstreamEndpointError::UnsupportedScheme);
        }

        Ok(Self { base_url: trimmed })
    }

    /// Builds a full upstream URL for a Codex request path.
    #[must_use]
    pub fn url_for_path(&self, request_path: &str) -> String {
        if self.base_url.ends_with("/backend-api") {
            return self.chatgpt_backend_url_for_path(request_path);
        }

        let relative_path = request_path.trim_start_matches('/');
        let upstream_path = relative_path
            .strip_prefix("v1/")
            .or_else(|| relative_path.strip_prefix("v1?"))
            .map_or(relative_path, |path| path);

        if let Some(query) = relative_path.strip_prefix("v1?") {
            return format!("{}?{query}", self.base_url);
        }
        if upstream_path.is_empty() || upstream_path == "v1" {
            return self.base_url.clone();
        }

        format!("{}/{}", self.base_url, upstream_path)
    }

    fn chatgpt_backend_url_for_path(&self, request_path: &str) -> String {
        let (path, query) = request_path.trim_start_matches('/').split_once('?').map_or(
            (request_path.trim_start_matches('/'), None),
            |(path, query)| (path, Some(query)),
        );
        let upstream_path = match path {
            "v1/responses" => "codex/responses",
            "v1/responses/compact" => "codex/responses/compact",
            _ => path.strip_prefix("v1/").unwrap_or(path),
        };
        let url = if upstream_path.is_empty() || upstream_path == "v1" {
            self.base_url.clone()
        } else {
            format!("{}/{}", self.base_url, upstream_path)
        };

        match query {
            Some(query) => format!("{url}?{query}"),
            None => url,
        }
    }

    /// Builds a WebSocket URL for a Codex request path.
    #[must_use]
    pub fn websocket_url_for_path(&self, request_path: &str) -> String {
        let http_url = self.url_for_path(request_path);
        if let Some(rest) = http_url.strip_prefix("https://") {
            return format!("wss://{rest}");
        }
        if let Some(rest) = http_url.strip_prefix("http://") {
            return format!("ws://{rest}");
        }

        http_url
    }
}

/// Upstream endpoint validation error.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum UpstreamEndpointError {
    /// Base URL was empty.
    #[error("upstream endpoint base URL is empty")]
    Empty,
    /// Base URL did not use HTTP or HTTPS.
    #[error("upstream endpoint base URL must use http or https")]
    UnsupportedScheme,
}

/// Blocking HTTP/1.1 upstream transport for local/mock HTTP endpoints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpUpstreamTransport {
    endpoint: UpstreamEndpoint,
}

impl HttpUpstreamTransport {
    /// Creates an HTTP upstream transport.
    #[must_use]
    pub const fn new(endpoint: UpstreamEndpoint) -> Self {
        Self { endpoint }
    }
}

impl UpstreamHttpTransport for HttpUpstreamTransport {
    fn send(&self, request: UpstreamHttpRequest) -> Result<HttpProxyResponse, HttpProxyError> {
        self.send_streaming(request)?.into_buffered()
    }
}

impl StreamingUpstreamHttpTransport for HttpUpstreamTransport {
    fn send_streaming(
        &self,
        request: UpstreamHttpRequest,
    ) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
        if self.endpoint.base_url.starts_with("https://") {
            return send_https_request(&self.endpoint, request);
        }

        send_http_request(&self.endpoint, request)
    }
}

fn send_http_request(
    endpoint: &UpstreamEndpoint,
    request: UpstreamHttpRequest,
) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
    let target = ParsedHttpTarget::parse(endpoint, request.path())?;
    let mut stream = TcpStream::connect(target.address()).map_err(upstream_io_error)?;
    write_request(&mut stream, &target, &request)?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(upstream_io_error)?;

    parse_streaming_response(stream)
}

fn send_https_request(
    endpoint: &UpstreamEndpoint,
    request: UpstreamHttpRequest,
) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(reqwest_error)?;
    let method = reqwest_method(&request);
    let mut builder = client.request(method, endpoint.url_for_path(request.path()));
    for header in request.headers().as_slice() {
        builder = builder.header(header.name(), header.value());
    }
    let response = builder
        .body(request.body().to_vec())
        .send()
        .map_err(reqwest_error)?;
    let status = response.status().as_u16();
    let mut response_headers = Vec::new();
    for (name, value) in response.headers() {
        let value = value.to_str().map_err(|_error| HttpProxyError::Upstream {
            message: "upstream response header was not utf-8".to_owned(),
        })?;
        response_headers.push(Header::new(name.as_str(), value));
    }

    Ok(StreamingHttpProxyResponse::new(
        status,
        HeaderCollection::new(response_headers),
        Box::new(response),
    ))
}

struct ParsedHttpTarget {
    host: String,
    port: u16,
    path: String,
}

impl ParsedHttpTarget {
    fn parse(endpoint: &UpstreamEndpoint, request_path: &str) -> Result<Self, HttpProxyError> {
        let Some(authority_and_path) = endpoint.base_url.strip_prefix("http://") else {
            return Err(HttpProxyError::Upstream {
                message: "http upstream transport requires http endpoint".to_owned(),
            });
        };
        let (authority, base_path) = authority_and_path
            .split_once('/')
            .map_or((authority_and_path, ""), |(authority, path)| {
                (authority, path)
            });
        let (host, port) = parse_authority(authority)?;
        let request_url = endpoint.url_for_path(request_path);
        let path = request_url
            .strip_prefix(&format!("http://{authority}"))
            .filter(|path| !path.is_empty())
            .map_or_else(
                || format!("/{base_path}"),
                |path| {
                    if path.starts_with('/') || path.starts_with('?') {
                        path.to_owned()
                    } else {
                        format!("/{path}")
                    }
                },
            );

        Ok(Self { host, port, path })
    }

    fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn parse_authority(authority: &str) -> Result<(String, u16), HttpProxyError> {
    let (host, port_text) = authority
        .rsplit_once(':')
        .ok_or_else(|| HttpProxyError::Upstream {
            message: "http upstream endpoint must include port".to_owned(),
        })?;
    let port = port_text
        .parse::<u16>()
        .map_err(|_error| HttpProxyError::Upstream {
            message: "http upstream endpoint port is invalid".to_owned(),
        })?;

    Ok((host.to_owned(), port))
}

fn write_request(
    stream: &mut TcpStream,
    target: &ParsedHttpTarget,
    request: &UpstreamHttpRequest,
) -> Result<(), HttpProxyError> {
    write!(
        stream,
        "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\n",
        method_text(request),
        target.path,
        target.host,
        request.body().len()
    )
    .map_err(upstream_io_error)?;
    stream
        .write_all(b"Connection: close\r\n")
        .map_err(upstream_io_error)?;
    for header in request.headers().as_slice() {
        write!(stream, "{}: {}\r\n", header.name(), header.value()).map_err(upstream_io_error)?;
    }
    stream.write_all(b"\r\n").map_err(upstream_io_error)?;
    stream.write_all(request.body()).map_err(upstream_io_error)
}

fn method_text(request: &UpstreamHttpRequest) -> &'static str {
    match request.method() {
        crate::routes::Method::Get => "GET",
        crate::routes::Method::Post => "POST",
        crate::routes::Method::Other => "POST",
    }
}

fn reqwest_method(request: &UpstreamHttpRequest) -> reqwest::Method {
    match request.method() {
        crate::routes::Method::Get => reqwest::Method::GET,
        crate::routes::Method::Post | crate::routes::Method::Other => reqwest::Method::POST,
    }
}

fn parse_streaming_response(
    mut stream: TcpStream,
) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
    let mut response_bytes = Vec::new();
    let header_length = loop {
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut response = httparse::Response::new(&mut headers);
        match response.parse(&response_bytes) {
            Ok(httparse::Status::Complete(header_length)) => break header_length,
            Ok(httparse::Status::Partial) => {}
            Err(_error) => {
                return Err(HttpProxyError::Upstream {
                    message: "failed parsing upstream response".to_owned(),
                });
            }
        }

        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).map_err(upstream_io_error)?;
        if read == 0 {
            return Err(HttpProxyError::Upstream {
                message: "partial upstream response".to_owned(),
            });
        }
        response_bytes.extend_from_slice(&buffer[..read]);
    };

    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut response = httparse::Response::new(&mut headers);
    match response
        .parse(&response_bytes)
        .map_err(|_error| HttpProxyError::Upstream {
            message: "failed parsing upstream response".to_owned(),
        })? {
        httparse::Status::Complete(_header_length) => {}
        httparse::Status::Partial => {
            return Err(HttpProxyError::Upstream {
                message: "partial upstream response".to_owned(),
            });
        }
    }
    let status = response.code.ok_or_else(|| HttpProxyError::Upstream {
        message: "upstream response missing status".to_owned(),
    })?;
    let mut response_headers = Vec::new();
    for header in response.headers.iter() {
        let value =
            std::str::from_utf8(header.value).map_err(|_error| HttpProxyError::Upstream {
                message: "upstream response header was not utf-8".to_owned(),
            })?;
        response_headers.push(Header::new(header.name, value));
    }
    let body_prefix = response_bytes[header_length..].to_vec();

    Ok(StreamingHttpProxyResponse::new(
        status,
        HeaderCollection::new(response_headers),
        Box::new(PrefixThenReader::new(body_prefix, stream)),
    ))
}

struct PrefixThenReader<R> {
    prefix: Cursor<Vec<u8>>,
    rest: R,
}

impl<R> PrefixThenReader<R> {
    fn new(prefix: Vec<u8>, rest: R) -> Self {
        Self {
            prefix: Cursor::new(prefix),
            rest,
        }
    }
}

impl<R> Read for PrefixThenReader<R>
where
    R: Read,
{
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let prefix_read = self.prefix.read(output)?;
        if prefix_read > 0 {
            return Ok(prefix_read);
        }

        self.rest.read(output)
    }
}

fn upstream_io_error(error: std::io::Error) -> HttpProxyError {
    HttpProxyError::Upstream {
        message: error.to_string(),
    }
}

fn reqwest_error(error: reqwest::Error) -> HttpProxyError {
    HttpProxyError::Upstream {
        message: error.to_string(),
    }
}

/// Builder for an upstream request.
#[derive(Clone, Debug)]
pub struct UpstreamRequestBuilder {
    route_kind: RouteKind,
    headers: Vec<Header>,
    body: Vec<u8>,
}

impl UpstreamRequestBuilder {
    /// Creates a builder.
    #[must_use]
    pub const fn new(route_kind: RouteKind) -> Self {
        Self {
            route_kind,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Adds a client header.
    #[must_use]
    pub fn with_header(mut self, header: Header) -> Self {
        self.headers.push(header);
        self
    }

    /// Sets the request body bytes.
    #[must_use]
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    /// Builds the upstream request.
    #[must_use]
    pub fn build(self, upstream_auth_token: SecretString) -> UpstreamRequest {
        self.build_with_chatgpt_account_id(upstream_auth_token, None)
    }

    /// Builds the upstream request with a ChatGPT account id header.
    #[must_use]
    pub fn build_with_chatgpt_account_id(
        self,
        upstream_auth_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> UpstreamRequest {
        UpstreamRequest {
            route_kind: self.route_kind,
            headers: sanitize_headers_for_upstream(
                self.headers,
                upstream_auth_token,
                chatgpt_account_id,
            ),
            body: self.body,
        }
    }
}

/// Upstream request after local sanitization and selected-account auth injection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamRequest {
    route_kind: RouteKind,
    headers: HeaderCollection,
    body: Vec<u8>,
}

impl UpstreamRequest {
    /// Returns route kind.
    #[must_use]
    pub const fn route_kind(&self) -> RouteKind {
        self.route_kind
    }

    /// Returns sanitized headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderCollection {
        &self.headers
    }

    /// Returns body bytes unchanged.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}
