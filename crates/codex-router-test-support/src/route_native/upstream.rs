use std::collections::BTreeMap;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use tungstenite::Message;
use tungstenite::accept_hdr;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;

use super::fixture::LOCAL_TOKEN;
use super::fixture::contains_route_native_upstream_token;
use super::fixture::join_result;
use super::fixture::selected_account_label_from_authorization;

pub(super) struct RouteNativeUpstream {
    address: SocketAddr,
    transcript: Arc<Mutex<Vec<RouteNativeRecordedRequest>>>,
    handle: thread::JoinHandle<Result<(), String>>,
}

impl RouteNativeUpstream {
    pub(super) fn start(expected_connections: usize) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("failed to bind route-native upstream: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("failed to read route-native upstream address: {error}"))?;
        let transcript = Arc::new(Mutex::new(Vec::new()));
        let thread_transcript = Arc::clone(&transcript);
        let handle = thread::Builder::new()
            .name("codex-router-route-native-upstream".to_owned())
            .spawn(move || {
                run_route_native_upstream(listener, thread_transcript, expected_connections)
            })
            .map_err(|error| format!("failed to spawn route-native upstream: {error}"))?;
        Ok(Self {
            address,
            transcript,
            handle,
        })
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(super) fn join(self) -> Result<RouteNativeTranscript, String> {
        join_result(self.handle, "route-native upstream")?;
        let transcript = self
            .transcript
            .lock()
            .map_err(|_| "route-native transcript mutex poisoned".to_owned())?
            .clone();
        Ok(RouteNativeTranscript::new(transcript))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteNativeTranscript {
    requests: Vec<RouteNativeRecordedRequest>,
}

impl RouteNativeTranscript {
    fn new(requests: Vec<RouteNativeRecordedRequest>) -> Self {
        Self { requests }
    }

    /// Returns recorded upstream requests.
    #[must_use]
    pub fn requests(&self) -> &[RouteNativeRecordedRequest] {
        &self.requests
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteNativeRecordedRequest {
    method: String,
    path: String,
    authorization_present: bool,
    selected_account_label: Option<String>,
    local_auth_header_present: bool,
    body: String,
}

impl RouteNativeRecordedRequest {
    /// Returns the recorded upstream method.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Returns the recorded upstream path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns whether upstream Authorization was present.
    #[must_use]
    pub const fn authorization_present(&self) -> bool {
        self.authorization_present
    }

    /// Returns the safe selected account label inferred from upstream auth.
    #[must_use]
    pub fn selected_account_label(&self) -> Option<&str> {
        self.selected_account_label.as_deref()
    }

    /// Returns whether local router auth leaked upstream.
    #[must_use]
    pub const fn local_auth_header_present(&self) -> bool {
        self.local_auth_header_present
    }
}

pub(super) fn assert_route_native_transcript(
    transcript: &RouteNativeTranscript,
) -> Result<(), String> {
    if transcript.requests().len() != 5 {
        return Err(format!(
            "route-native upstream should see exactly 5 successful routed requests, saw {}",
            transcript.requests().len()
        ));
    }
    let mut seen = BTreeMap::new();
    for request in transcript.requests() {
        if !request.authorization_present() {
            return Err(format!(
                "route-native upstream request {} {} missed upstream authorization",
                request.method(),
                request.path()
            ));
        }
        if request.local_auth_header_present() || request.body.contains(LOCAL_TOKEN) {
            return Err(format!(
                "route-native upstream request {} {} leaked local auth",
                request.method(),
                request.path()
            ));
        }
        if contains_route_native_upstream_token(&request.body)
            || request.body.contains("router_affinity_hash_secret")
        {
            return Err(format!(
                "route-native upstream request {} {} leaked secret material",
                request.method(),
                request.path()
            ));
        }
        seen.insert(
            (request.method().to_owned(), request.path().to_owned()),
            request.selected_account_label().map(str::to_owned),
        );
    }
    for expected in [
        (
            "POST".to_owned(),
            "/v1/responses".to_owned(),
            "route-responses",
        ),
        ("GET".to_owned(), "/v1/models".to_owned(), "route-models"),
        (
            "POST".to_owned(),
            "/v1/memories/trace_summarize".to_owned(),
            "route-memories",
        ),
        (
            "POST".to_owned(),
            "/v1/responses/compact".to_owned(),
            "route-compact",
        ),
        (
            "WEBSOCKET".to_owned(),
            "/v1/responses".to_owned(),
            "route-responses",
        ),
    ] {
        match seen.get(&(expected.0.clone(), expected.1.clone())) {
            Some(Some(label)) if label == expected.2 => {}
            Some(label) => {
                return Err(format!(
                    "route-native upstream selected wrong account for {} {}: expected {}, got {label:?}",
                    expected.0, expected.1, expected.2
                ));
            }
            None => {
                return Err(format!(
                    "route-native upstream missed expected request {} {}",
                    expected.0, expected.1
                ));
            }
        }
    }

    Ok(())
}

fn run_route_native_upstream(
    listener: TcpListener,
    transcript: Arc<Mutex<Vec<RouteNativeRecordedRequest>>>,
    expected_connections: usize,
) -> Result<(), String> {
    for _ in 0..expected_connections {
        let (mut stream, _) = listener
            .accept()
            .map_err(|error| format!("route-native upstream accept failed: {error}"))?;
        if looks_like_websocket_upgrade(&stream)? {
            handle_upstream_websocket(stream, &transcript)?;
        } else {
            handle_upstream_http(&mut stream, &transcript)?;
        }
    }
    Ok(())
}

fn handle_upstream_http(
    stream: &mut TcpStream,
    transcript: &Arc<Mutex<Vec<RouteNativeRecordedRequest>>>,
) -> Result<(), String> {
    let raw_request = read_http_request(stream)?;
    let recorded = recorded_from_http_request(&raw_request)?;
    let body = response_body_for_path(recorded.path.as_str());
    transcript
        .lock()
        .map_err(|_| "route-native transcript mutex poisoned".to_owned())?
        .push(recorded);
    let content_type = if body.starts_with("data:") {
        "text/event-stream"
    } else {
        "application/json"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| format!("route-native upstream failed to write HTTP response: {error}"))
}

#[allow(clippy::result_large_err)]
fn handle_upstream_websocket(
    stream: TcpStream,
    transcript: &Arc<Mutex<Vec<RouteNativeRecordedRequest>>>,
) -> Result<(), String> {
    let headers = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let request_headers = Arc::clone(&headers);
    let callback = move |request: &Request, response: Response| {
        if let Ok(mut headers) = request_headers.lock() {
            for (name, value) in request.headers() {
                headers.push((
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or("<non-utf8>").to_owned(),
                ));
            }
        }
        Ok(response)
    };
    let mut websocket = accept_hdr(stream, callback)
        .map_err(|error| format!("route-native upstream websocket accept failed: {error}"))?;
    let first_frame = websocket
        .read()
        .map_err(|error| format!("route-native upstream websocket read failed: {error}"))?
        .to_string();
    let headers = headers
        .lock()
        .map_err(|_| "route-native websocket header mutex poisoned".to_owned())?
        .clone();
    let authorization_present = headers.iter().any(|(name, value)| {
        name == "authorization" && selected_account_label_from_authorization(value).is_some()
    });
    let selected_account_label = headers.iter().find_map(|(name, value)| {
        if name == "authorization" {
            selected_account_label_from_authorization(value).map(str::to_owned)
        } else {
            None
        }
    });
    let local_auth_header_present = headers
        .iter()
        .any(|(name, value)| name == "x-codex-router-token" || value.contains(LOCAL_TOKEN));
    transcript
        .lock()
        .map_err(|_| "route-native transcript mutex poisoned".to_owned())?
        .push(RouteNativeRecordedRequest {
            method: "WEBSOCKET".to_owned(),
            path: "/v1/responses".to_owned(),
            authorization_present,
            selected_account_label,
            local_auth_header_present,
            body: first_frame,
        });
    websocket
        .send(Message::text(r#"{"type":"response.completed"}"#))
        .map_err(|error| format!("route-native upstream websocket write failed: {error}"))?;
    Ok(())
}

fn response_body_for_path(path: &str) -> &'static str {
    match path {
        "/v1/responses" => "data: {\"type\":\"response.completed\"}\n\n",
        "/v1/models" => r#"{"object":"list","data":[{"id":"gpt-route-native"}]}"#,
        "/v1/memories/trace_summarize" => r#"{"summary":"route-native"}"#,
        "/v1/responses/compact" => r#"{"id":"resp-compact-route-native"}"#,
        _ => r#"{"ok":true}"#,
    }
}

fn read_http_request(stream: &mut TcpStream) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let byte_count = stream.read(&mut buffer).map_err(|error| {
            format!("route-native upstream failed to read HTTP request: {error}")
        })?;
        if byte_count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..byte_count]);
        if let Some(header_end) = find_header_end(&bytes) {
            let header_text = String::from_utf8_lossy(&bytes[..header_end]).to_string();
            let content_length = parse_content_length(&header_text);
            let body_start = header_end + 4;
            if bytes.len() >= body_start + content_length {
                return String::from_utf8(bytes[..body_start + content_length].to_vec())
                    .map_err(|error| format!("route-native HTTP request was not UTF-8: {error}"));
            }
        }
    }
    Err("route-native upstream received incomplete HTTP request".to_owned())
}

fn recorded_from_http_request(raw_request: &str) -> Result<RouteNativeRecordedRequest, String> {
    let (head, body) = raw_request
        .split_once("\r\n\r\n")
        .ok_or_else(|| "route-native HTTP request had no header delimiter".to_owned())?;
    let mut lines = head.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| "route-native HTTP request missed request line".to_owned())?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "route-native HTTP request missed method".to_owned())?
        .to_owned();
    let path = parts
        .next()
        .ok_or_else(|| "route-native HTTP request missed path".to_owned())?
        .split('?')
        .next()
        .unwrap_or("")
        .to_owned();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect::<Vec<_>>();
    let authorization_present = headers.iter().any(|(name, value)| {
        name == "authorization" && selected_account_label_from_authorization(value).is_some()
    });
    let selected_account_label = headers.iter().find_map(|(name, value)| {
        if name == "authorization" {
            selected_account_label_from_authorization(value).map(str::to_owned)
        } else {
            None
        }
    });
    let local_auth_header_present = headers
        .iter()
        .any(|(name, value)| name == "x-codex-router-token" || value.contains(LOCAL_TOKEN));

    Ok(RouteNativeRecordedRequest {
        method,
        path,
        authorization_present,
        selected_account_label,
        local_auth_header_present,
        body: body.to_owned(),
    })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header_text: &str) -> usize {
    header_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

fn looks_like_websocket_upgrade(stream: &TcpStream) -> Result<bool, String> {
    let mut buffer = [0_u8; 1024];
    let byte_count = stream
        .peek(&mut buffer)
        .map_err(|error| format!("route-native upstream failed to peek request: {error}"))?;
    let request = String::from_utf8_lossy(&buffer[..byte_count]);
    Ok(request.to_ascii_lowercase().contains("upgrade: websocket"))
}
