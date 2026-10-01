use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const SUCCESS_MODE: u8 = 0;
const USAGE_LIMIT_MODE: u8 = 1;
pub(super) const USAGE_LIMIT_PROMPT_MARKER: &str = "CLAUDE-CLIENT-EXHAUSTED-PROMPT";

pub(super) struct FakeAnthropicServer {
    address: SocketAddr,
    response_mode: Arc<AtomicU8>,
    stopping: Arc<AtomicBool>,
    requests: mpsc::Receiver<ObservedRequest>,
    worker: Option<JoinHandle<()>>,
}

pub(super) struct ObservedRequest {
    pub(super) path: String,
    pub(super) has_synthetic_account_token: bool,
    pub(super) contains_new_prompt: bool,
    pub(super) contains_resume_prompt: bool,
    pub(super) contains_acp_prompt: bool,
    pub(super) contains_usage_limit_prompt: bool,
    pub(super) received_usage_limit: bool,
}

pub(super) fn is_messages_request_target(request_target: &str) -> bool {
    request_target
        .split_once('?')
        .map_or(request_target, |(path, _query)| path)
        == "/v1/messages"
}

impl FakeAnthropicServer {
    pub(super) fn start(
        address: SocketAddr,
        expected_account_token: &'static str,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(false)?;
        let address = listener.local_addr()?;
        let response_mode = Arc::new(AtomicU8::new(SUCCESS_MODE));
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_mode = Arc::clone(&response_mode);
        let worker_stopping = Arc::clone(&stopping);
        let (requests_tx, requests_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("claude-fake-anthropic".to_owned())
            .spawn(move || {
                run_fake_anthropic_server(
                    listener,
                    expected_account_token,
                    worker_mode,
                    worker_stopping,
                    requests_tx,
                );
            })?;

        Ok(Self {
            address,
            response_mode,
            stopping,
            requests: requests_rx,
            worker: Some(worker),
        })
    }

    pub(super) fn return_usage_limit(&self) {
        self.response_mode.store(USAGE_LIMIT_MODE, Ordering::SeqCst);
    }

    pub(super) fn next_request(
        &self,
        timeout: Duration,
    ) -> Result<ObservedRequest, mpsc::RecvTimeoutError> {
        self.requests.recv_timeout(timeout)
    }

    pub(super) fn drain_requests(&self) -> Vec<ObservedRequest> {
        self.requests.try_iter().collect()
    }
}

impl Drop for FakeAnthropicServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let _wake_accept = TcpStream::connect(self.address);
        if let Some(worker) = self.worker.take() {
            let _joined = worker.join();
        }
    }
}

fn run_fake_anthropic_server(
    listener: TcpListener,
    expected_account_token: &str,
    response_mode: Arc<AtomicU8>,
    stopping: Arc<AtomicBool>,
    requests: mpsc::Sender<ObservedRequest>,
) {
    while !stopping.load(Ordering::SeqCst) {
        let (mut stream, _) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        if stopping.load(Ordering::SeqCst) {
            return;
        }
        let _timeout = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok(request) = read_request(&mut stream) else {
            continue;
        };
        let usage_limit = response_mode.load(Ordering::SeqCst) == USAGE_LIMIT_MODE
            && request.body.contains(USAGE_LIMIT_PROMPT_MARKER);
        let observation = ObservedRequest {
            path: request.path.clone(),
            has_synthetic_account_token: request.authorization.as_deref().is_some_and(|value| {
                value
                    .strip_prefix("Bearer ")
                    .is_some_and(|token| token == expected_account_token)
            }),
            contains_new_prompt: request.body.contains("CLAUDE-CLIENT-NEW-PROMPT"),
            contains_resume_prompt: request.body.contains("CLAUDE-CLIENT-RESUME-PROMPT"),
            contains_acp_prompt: request.body.contains("CLAUDE-ACP-HOST-PROMPT"),
            contains_usage_limit_prompt: request.body.contains(USAGE_LIMIT_PROMPT_MARKER),
            received_usage_limit: usage_limit,
        };
        let _sent = requests.send(observation);
        if !is_messages_request_target(&request.path) {
            let _response = write_response(&mut stream, 404, "application/json", "{}", &[]);
        } else if usage_limit {
            let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Synthetic usage limit reached; retry after 90 seconds."}}"#;
            let reset_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs().saturating_add(90));
            let headers = [
                ("anthropic-ratelimit-unified-status", "rejected".to_owned()),
                (
                    "anthropic-ratelimit-unified-representative-claim",
                    "five_hour".to_owned(),
                ),
                (
                    "anthropic-ratelimit-unified-5h-status",
                    "rejected".to_owned(),
                ),
                ("anthropic-ratelimit-unified-5h-reset", reset_at.to_string()),
            ];
            let _response = write_response(&mut stream, 429, "application/json", body, &headers);
        } else {
            let _response = write_response(
                &mut stream,
                200,
                "text/event-stream",
                success_event_stream(),
                &[("cache-control", "no-cache".to_owned())],
            );
        }
    }
}

struct ParsedRequest {
    path: String,
    authorization: Option<String>,
    body: String,
}

fn read_request(stream: &mut TcpStream) -> io::Result<ParsedRequest> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request too large",
            ));
        }
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        let mut chunk = [0_u8; 8192];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request headers ended",
            ));
        }
        let read_bytes = chunk
            .get(..read)
            .ok_or_else(|| io::Error::other("invalid request read length"))?;
        bytes.extend_from_slice(read_bytes);
    };

    let header_bytes = bytes
        .get(..header_end)
        .ok_or_else(|| io::Error::other("request header boundary is invalid"))?;
    let header_text = std::str::from_utf8(header_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request headers are not UTF-8"))?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "request line is missing"))?;
    let mut request_fields = request_line.split_ascii_whitespace();
    let _method = request_fields
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "method is missing"))?;
    let path = request_fields
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "path is missing"))?
        .to_owned();

    let mut content_length = 0_usize;
    let mut authorization = None;
    for header in lines {
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid content length")
            })?;
        } else if name.eq_ignore_ascii_case("authorization") {
            authorization = Some(value.trim().to_owned());
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value.trim().eq_ignore_ascii_case("chunked")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "chunked upstream requests are not supported by this fixture",
            ));
        }
    }
    if content_length > MAX_REQUEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request body too large",
        ));
    }
    while bytes.len() - header_end < content_length {
        let mut chunk = [0_u8; 8192];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request body ended",
            ));
        }
        let read_bytes = chunk
            .get(..read)
            .ok_or_else(|| io::Error::other("invalid request body read length"))?;
        bytes.extend_from_slice(read_bytes);
    }
    let body_end = header_end + content_length;
    let body_bytes = bytes
        .get(header_end..body_end)
        .ok_or_else(|| io::Error::other("request body boundary is invalid"))?;
    let body = String::from_utf8_lossy(body_bytes).into_owned();

    Ok(ParsedRequest {
        path,
        authorization,
        body,
    })
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    headers: &[(&str, String)],
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        429 => "Too Many Requests",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    )?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn success_event_stream() -> &'static str {
    concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_acceptance\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"CLAUDE-ACCEPTANCE-OK\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    )
}
