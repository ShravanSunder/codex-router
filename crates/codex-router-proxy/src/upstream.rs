//! Upstream request construction.

#[cfg(test)]
use std::io::Cursor;
#[cfg(test)]
use std::io::Read;
#[cfg(test)]
use std::io::Write;
#[cfg(test)]
use std::net::Shutdown;
#[cfg(test)]
use std::net::TcpStream;

use bytes::Bytes;
use codex_router_core::redaction::SecretString;
use futures_util::future::BoxFuture;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper_rustls::HttpsConnector;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use thiserror::Error;

use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::headers::sanitize_headers_for_upstream;
use crate::http_sse::AsyncHttpBodyError;
use crate::http_sse::AsyncStreamingHttpProxyResponse;
use crate::http_sse::AsyncStreamingUpstreamHttpTransport;
use crate::http_sse::HttpProxyError;
#[cfg(test)]
use crate::http_sse::HttpProxyResponse;
#[cfg(test)]
use crate::http_sse::StreamingHttpProxyResponse;
use crate::http_sse::StreamingUpstreamHttpRequest;
#[cfg(test)]
use crate::http_sse::StreamingUpstreamHttpTransport;
#[cfg(test)]
use crate::http_sse::UpstreamHttpRequest;
#[cfg(test)]
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
        let normalized_path = request_path.trim_start_matches('/');
        let (path, query) = normalized_path
            .split_once('?')
            .map_or((normalized_path, None), |(path, query)| (path, Some(query)));
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
#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpUpstreamTransport {
    endpoint: UpstreamEndpoint,
}

#[cfg(test)]
impl HttpUpstreamTransport {
    /// Creates an HTTP upstream transport.
    #[must_use]
    pub const fn new(endpoint: UpstreamEndpoint) -> Self {
        Self { endpoint }
    }
}

#[cfg(test)]
impl UpstreamHttpTransport for HttpUpstreamTransport {
    fn send(&self, request: UpstreamHttpRequest) -> Result<HttpProxyResponse, HttpProxyError> {
        self.send_streaming(request)?.into_buffered()
    }
}

#[cfg(test)]
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

/// Hyper-backed async HTTP/SSE upstream transport.
#[derive(Clone)]
pub struct HyperHttpUpstreamTransport {
    endpoint: UpstreamEndpoint,
    client: Client<HttpsConnector<HttpConnector>, BoxBody<Bytes, AsyncHttpBodyError>>,
}

impl HyperHttpUpstreamTransport {
    /// Creates a Hyper HTTP upstream transport.
    #[must_use]
    pub fn new(endpoint: UpstreamEndpoint) -> Self {
        let connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();
        let client = Client::builder(TokioExecutor::new()).build(connector);

        Self { endpoint, client }
    }

    async fn send_streaming_inner(
        &self,
        request: StreamingUpstreamHttpRequest,
    ) -> Result<AsyncStreamingHttpProxyResponse, HttpProxyError> {
        let uri = self
            .endpoint
            .url_for_path(request.path())
            .parse::<http::Uri>()
            .map_err(|_error| HttpProxyError::Upstream {
                message: "upstream URI was invalid".to_owned(),
            })?;
        let mut builder = http::Request::builder()
            .method(hyper_method(request.method()))
            .uri(uri);
        for header in request.headers().as_slice() {
            builder = builder.header(header.name(), header.value());
        }
        let request =
            builder
                .body(request.into_body())
                .map_err(|_error| HttpProxyError::Upstream {
                    message: "failed building upstream request".to_owned(),
                })?;
        let response =
            self.client
                .request(request)
                .await
                .map_err(|error| HttpProxyError::Upstream {
                    message: error.to_string(),
                })?;
        let status = response.status().as_u16();
        let headers = response_headers(response.headers())?;
        let body = response.into_body().map_err(incoming_body_error).boxed();

        Ok(AsyncStreamingHttpProxyResponse::new(
            status,
            HeaderCollection::new(headers),
            body,
        ))
    }
}

impl AsyncStreamingUpstreamHttpTransport for HyperHttpUpstreamTransport {
    fn send_streaming<'a>(
        &'a self,
        request: StreamingUpstreamHttpRequest,
    ) -> BoxFuture<'a, Result<AsyncStreamingHttpProxyResponse, HttpProxyError>> {
        Box::pin(async move { self.send_streaming_inner(request).await })
    }
}

fn response_headers(headers: &http::HeaderMap) -> Result<Vec<Header>, HttpProxyError> {
    let mut response_headers = Vec::new();
    for (name, value) in headers {
        let value = value.to_str().map_err(|_error| HttpProxyError::Upstream {
            message: "upstream response header was not utf-8".to_owned(),
        })?;
        response_headers.push(Header::new(name.as_str(), value));
    }

    Ok(response_headers)
}

fn incoming_body_error(error: hyper::Error) -> AsyncHttpBodyError {
    Box::new(error)
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
struct ParsedHttpTarget {
    host: String,
    port: u16,
    path: String,
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
fn method_text(request: &UpstreamHttpRequest) -> &'static str {
    match request.method() {
        crate::routes::Method::Get => "GET",
        crate::routes::Method::Post => "POST",
        crate::routes::Method::Other => "POST",
    }
}

#[cfg(test)]
fn reqwest_method(request: &UpstreamHttpRequest) -> reqwest::Method {
    match request.method() {
        crate::routes::Method::Get => reqwest::Method::GET,
        crate::routes::Method::Post | crate::routes::Method::Other => reqwest::Method::POST,
    }
}

fn hyper_method(method: crate::routes::Method) -> http::Method {
    match method {
        crate::routes::Method::Get => http::Method::GET,
        crate::routes::Method::Post | crate::routes::Method::Other => http::Method::POST,
    }
}

#[cfg(test)]
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

#[cfg(test)]
struct PrefixThenReader<R> {
    prefix: Cursor<Vec<u8>>,
    rest: R,
}

#[cfg(test)]
impl<R> PrefixThenReader<R> {
    fn new(prefix: Vec<u8>, rest: R) -> Self {
        Self {
            prefix: Cursor::new(prefix),
            rest,
        }
    }
}

#[cfg(test)]
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

#[cfg(test)]
fn upstream_io_error(error: std::io::Error) -> HttpProxyError {
    HttpProxyError::Upstream {
        message: error.to_string(),
    }
}

#[cfg(test)]
fn reqwest_error(error: reqwest::Error) -> HttpProxyError {
    HttpProxyError::Upstream {
        message: error.to_string(),
    }
}

#[cfg(test)]
#[tokio::test]
async fn hyper_http_upstream_transport_passes_backend_api_models_upstream() {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("mock upstream should bind: {error}"),
    };
    let server_address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock upstream address should be readable: {error}"),
    };
    let server_task = tokio::spawn(async move {
        let (mut stream, _peer_address) = match listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept one connection: {error}"),
        };
        let mut request_bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = match stream.read(&mut buffer).await {
                Ok(read) => read,
                Err(error) => panic!("mock upstream should read request: {error}"),
            };
            if read == 0 {
                break;
            }
            request_bytes.extend_from_slice(&buffer[..read]);
            if request_bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let body = br#"{"object":"list","data":[{"id":"upstream-model"}]}"#;
        let response_head = format!(
            "HTTP/1.1 203 Non-Authoritative Information\r\n\
             ETag: upstream-models-etag\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n",
            body.len()
        );
        if let Err(error) = stream.write_all(response_head.as_bytes()).await {
            panic!("mock upstream should write response head: {error}");
        }
        if let Err(error) = stream.write_all(body).await {
            panic!("mock upstream should write response body: {error}");
        }

        String::from_utf8(request_bytes)
            .unwrap_or_else(|error| panic!("mock upstream request should be utf-8: {error}"))
    });
    let endpoint = match UpstreamEndpoint::new(format!("http://{server_address}/backend-api")) {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("mock endpoint should validate: {error}"),
    };
    let upstream = HyperHttpUpstreamTransport::new(endpoint);
    let request = UpstreamHttpRequest::new_for_test(
        crate::routes::Method::Get,
        "/v1/models".to_owned(),
        crate::routes::RouteKind::Models,
        HeaderCollection::new(vec![Header::new(
            "Authorization",
            "Bearer selected-upstream-token",
        )]),
        Vec::new(),
    )
    .into_streaming_body(
        http_body_util::Full::new(Bytes::new())
            .map_err(|never| -> AsyncHttpBodyError { match never {} })
            .boxed(),
    );

    let response =
        match AsyncStreamingUpstreamHttpTransport::send_streaming(&upstream, request).await {
            Ok(response) => response,
            Err(error) => panic!("Hyper upstream transport should forward request: {error}"),
        };
    let (status, headers, body) = response.into_parts();
    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => panic!("Hyper upstream response body should collect: {error}"),
    };
    let recorded_request = match server_task.await {
        Ok(request) => request,
        Err(error) => panic!("mock upstream task should complete: {error}"),
    };
    let normalized_request = recorded_request.to_ascii_lowercase();

    assert_eq!(status, 203);
    assert_eq!(headers.value("etag"), Some("upstream-models-etag"));
    assert_eq!(
        body_bytes.as_ref(),
        br#"{"object":"list","data":[{"id":"upstream-model"}]}"#
    );
    assert!(recorded_request.starts_with("GET /backend-api/models HTTP/1.1\r\n"));
    assert!(normalized_request.contains("authorization: bearer selected-upstream-token\r\n"));
    assert!(!recorded_request.contains(r#"{"models":[]}"#));
}

#[cfg(test)]
#[tokio::test]
async fn hyper_http_upstream_transport_streams_request_body_before_eof() {
    use http_body_util::StreamBody;
    use hyper::body::Frame;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::mpsc;
    use tokio::time::Duration;

    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("mock upstream should bind: {error}"),
    };
    let server_address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock upstream address should be readable: {error}"),
    };
    let (first_chunk_seen_sender, mut first_chunk_seen_receiver) = mpsc::channel(1);
    let server_task = tokio::spawn(async move {
        let (mut stream, _peer_address) = match listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept one connection: {error}"),
        };
        let mut request_bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = match stream.read(&mut buffer).await {
                Ok(read) => read,
                Err(error) => panic!("mock upstream should read request: {error}"),
            };
            if read == 0 {
                panic!("mock upstream request ended before first chunk");
            }
            request_bytes.extend_from_slice(&buffer[..read]);
            if request_bytes
                .windows("stream-first".len())
                .any(|window| window == b"stream-first")
            {
                if let Err(error) = first_chunk_seen_sender.send(()).await {
                    panic!("first chunk signal should send: {error}");
                }
                break;
            }
        }

        let response_head = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        if let Err(error) = stream.write_all(response_head.as_bytes()).await {
            panic!("mock upstream should write response: {error}");
        }
    });

    let endpoint = match UpstreamEndpoint::new(format!("http://{server_address}/v1")) {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("mock endpoint should validate: {error}"),
    };
    let upstream = HyperHttpUpstreamTransport::new(endpoint);
    let (body_sender, body_receiver) = mpsc::channel::<Result<Frame<Bytes>, AsyncHttpBodyError>>(2);
    let body_stream = futures_util::stream::unfold(body_receiver, |mut receiver| async {
        receiver.recv().await.map(|frame| (frame, receiver))
    });
    let request = StreamingUpstreamHttpRequest::new_for_test(
        crate::routes::Method::Post,
        "/v1/responses".to_owned(),
        crate::routes::RouteKind::Responses,
        HeaderCollection::new(vec![Header::new(
            "Authorization",
            "Bearer selected-upstream-token",
        )]),
        BodyExt::boxed(StreamBody::new(body_stream)),
    );

    if let Err(error) = body_sender
        .send(Ok(Frame::data(Bytes::from_static(b"stream-first"))))
        .await
    {
        panic!("first body chunk should send: {error}");
    }
    let response_task = tokio::spawn(async move { upstream.send_streaming(request).await });
    match tokio::time::timeout(Duration::from_secs(1), first_chunk_seen_receiver.recv()).await {
        Ok(Some(())) => {}
        Ok(None) => panic!("mock upstream first chunk signal closed"),
        Err(_elapsed) => panic!("upstream did not receive first body chunk before EOF"),
    }
    if let Err(error) = body_sender
        .send(Ok(Frame::data(Bytes::from_static(b"stream-second"))))
        .await
    {
        panic!("second body chunk should send: {error}");
    }
    drop(body_sender);

    let response = match response_task.await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => panic!("Hyper upstream transport should forward request: {error}"),
        Err(error) => panic!("Hyper upstream response task should join: {error}"),
    };
    let (status, _headers, body) = response.into_parts();
    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => panic!("Hyper upstream response body should collect: {error}"),
    };
    match server_task.await {
        Ok(()) => {}
        Err(error) => panic!("mock upstream task should complete: {error}"),
    }

    assert_eq!(status, 200);
    assert_eq!(body_bytes.as_ref(), b"ok");
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

    /// Builds the upstream request with optional ChatGPT account affinity.
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
