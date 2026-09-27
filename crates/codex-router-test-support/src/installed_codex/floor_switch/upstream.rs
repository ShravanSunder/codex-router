//! Fixture loopback upstream for installed-Codex floor journeys.

use std::io::ErrorKind;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;
use tungstenite::Message;
use tungstenite::WebSocket;
use tungstenite::accept_hdr;
use tungstenite::error::ProtocolError;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;

use super::super::QUOTA_RECONNECT_FALLBACK;
use super::super::QUOTA_RECONNECT_PRIMARY;
use super::super::bearer_token_from_headers;
use super::super::frame_contains_function_call_output;
use super::super::is_non_prewarm_response_create_frame;
use super::super::is_prewarm_request_frame;
use super::super::looks_like_websocket_upgrade;
use super::super::respond_to_http_request;
use super::super::smoke_prewarm_events;
use super::super::smoke_response_events;
use super::FIXTURE_WAIT;
use super::FloorJourney;

pub(super) struct FloorUpstream {
    pub(super) address: String,
    pub(super) first_turn_started: mpsc::Receiver<()>,
    pub(super) release_first_turn: mpsc::Sender<()>,
    worker: thread::JoinHandle<Result<FloorUpstreamObservation, String>>,
}

#[derive(Debug)]
pub(super) struct FloorUpstreamObservation {
    pub(super) first_account: &'static str,
    pub(super) followup_account: &'static str,
    pub(super) followed_on_new_connection: bool,
    pub(super) followup_contains_first_tool_output: bool,
    pub(super) forwarded_stale_response_id: bool,
}

impl FloorUpstream {
    pub(super) fn start(journey: FloorJourney) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("floor upstream bind failed: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("floor upstream nonblocking setup failed: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("floor upstream address failed: {error}"))?
            .to_string();
        let (started_sender, first_turn_started) = mpsc::channel();
        let (release_first_turn, release_receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(format!("codex-router-floor-upstream-{}", journey.name()))
            .spawn(move || run_floor_upstream(listener, journey, started_sender, release_receiver))
            .map_err(|error| format!("floor upstream thread failed: {error}"))?;
        Ok(Self {
            address,
            first_turn_started,
            release_first_turn,
            worker,
        })
    }

    pub(super) fn finish(self) -> Result<FloorUpstreamObservation, String> {
        self.worker
            .join()
            .map_err(|_| "floor upstream thread panicked".to_owned())?
    }
}

fn run_floor_upstream(
    listener: TcpListener,
    journey: FloorJourney,
    first_turn_started: mpsc::Sender<()>,
    release_first_turn: mpsc::Receiver<()>,
) -> Result<FloorUpstreamObservation, String> {
    let deadline = Instant::now() + FIXTURE_WAIT;
    let (mut first_socket, first_account, _first_frame) = accept_real_request(&listener, deadline)?;
    if first_account != "primary" {
        return Err("first installed-Codex turn did not reach the primary account".to_owned());
    }
    send_first_tool_call(&mut first_socket)?;
    first_turn_started
        .send(())
        .map_err(|_| "floor test controller disappeared before first turn".to_owned())?;
    release_first_turn
        .recv_timeout(FIXTURE_WAIT)
        .map_err(|_| "floor test controller did not release first turn".to_owned())?;

    let (followup_account, followup_frame, followed_on_new_connection, mut followup_socket) =
        if matches!(journey, FloorJourney::NoPeer) {
            send_first_turn_completed(&mut first_socket)?;
            let frame = read_real_request(&mut first_socket)?
                .ok_or_else(|| "no-peer journey disconnected before follow-up".to_owned())?;
            ("primary", frame, false, first_socket)
        } else {
            if matches!(journey, FloorJourney::HealthyPeer) {
                send_first_turn_completed(&mut first_socket)?;
            }
            expect_router_reconnect(&mut first_socket)?;
            let (socket, account, frame) = accept_real_request(&listener, deadline)?;
            (account, frame, true, socket)
        };
    for event in smoke_response_events(200) {
        followup_socket
            .send(Message::Text(event.into()))
            .map_err(|error| format!("floor upstream final response failed: {error}"))?;
    }
    let _ = followup_socket.close(None);
    let forwarded_stale_response_id = followup_frame
        .get("previous_response_id")
        .and_then(Value::as_str)
        == Some("resp-floor-first");
    let followup_contains_first_tool_output =
        frame_contains_function_call_output(&followup_frame.to_string(), "codex-router-floor-call");
    Ok(FloorUpstreamObservation {
        first_account,
        followup_account,
        followed_on_new_connection,
        followup_contains_first_tool_output,
        forwarded_stale_response_id,
    })
}

#[allow(clippy::result_large_err)]
fn accept_real_request(
    listener: &TcpListener,
    deadline: Instant,
) -> Result<(WebSocket<TcpStream>, &'static str, Value), String> {
    loop {
        if Instant::now() >= deadline {
            return Err("floor upstream timed out waiting for a real Codex request".to_owned());
        }
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => return Err(format!("floor upstream accept failed: {error}")),
        };
        stream
            .set_nonblocking(false)
            .map_err(|error| format!("floor upstream blocking mode failed: {error}"))?;
        stream
            .set_read_timeout(Some(FIXTURE_WAIT))
            .map_err(|error| format!("floor upstream read timeout failed: {error}"))?;
        if !looks_like_websocket_upgrade(&stream)? {
            respond_to_http_request(stream)?;
            continue;
        }
        let captured_headers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let callback_headers = std::sync::Arc::clone(&captured_headers);
        let mut socket = accept_hdr(stream, move |request: &Request, response: Response| {
            let headers = request
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.as_str().to_owned(), value.to_owned()))
                })
                .collect();
            if let Ok(mut captured) = callback_headers.lock() {
                *captured = headers;
            }
            Ok(response)
        })
        .map_err(|error| format!("floor upstream websocket handshake failed: {error}"))?;
        let account = {
            let headers = captured_headers
                .lock()
                .map_err(|_| "floor upstream header capture failed".to_owned())?;
            match bearer_token_from_headers(&headers) {
                Some(token) if token == QUOTA_RECONNECT_PRIMARY.upstream_token => "primary",
                Some(token) if token == QUOTA_RECONNECT_FALLBACK.upstream_token => "fallback",
                _ => return Err("floor upstream received an unknown fixture account".to_owned()),
            }
        };
        if let Some(frame) = read_real_request(&mut socket)? {
            return Ok((socket, account, frame));
        }
    }
}

fn read_real_request(socket: &mut WebSocket<TcpStream>) -> Result<Option<Value>, String> {
    let mut frame_shapes = Vec::new();
    loop {
        let frame = match socket.read() {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Binary(bytes)) => String::from_utf8(bytes.to_vec())
                .map_err(|_| "floor upstream received a non-UTF8 request".to_owned())?,
            Ok(Message::Close(_)) => return Ok(None),
            Ok(_) => continue,
            Err(tungstenite::Error::ConnectionClosed) => return Ok(None),
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                return Err(format!(
                    "floor upstream timed out after frame shapes {frame_shapes:?}"
                ));
            }
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(format!("floor upstream request read failed: {error}")),
        };
        if is_prewarm_request_frame(&frame) {
            if frame_shapes.len() < 8 {
                frame_shapes.push("prewarm".to_owned());
            }
            for event in smoke_prewarm_events(100) {
                socket
                    .send(Message::Text(event.into()))
                    .map_err(|error| format!("floor upstream prewarm failed: {error}"))?;
            }
            continue;
        }
        let value: Value = serde_json::from_str(&frame)
            .map_err(|_| "floor upstream received malformed Codex frame".to_owned())?;
        if frame_shapes.len() < 8 {
            let frame_kind = match value.get("type").and_then(Value::as_str) {
                Some("response.create") => "response.create",
                Some("session.update") => "session.update",
                _ => "other",
            };
            frame_shapes.push(format!(
                "{frame_kind}:model={},input_array={},stream={}",
                value.get("model").and_then(Value::as_str).is_some(),
                value.get("input").and_then(Value::as_array).is_some(),
                value.get("stream").and_then(Value::as_bool) == Some(true),
            ));
        }
        if is_non_prewarm_response_create_frame(&value) {
            return Ok(Some(value));
        }
    }
}

fn send_first_tool_call(socket: &mut WebSocket<TcpStream>) -> Result<(), String> {
    let events = [
        serde_json::json!({"type":"response.created","response":{"id":"resp-floor-first"}}),
        serde_json::json!({
            "type":"response.output_item.done",
            "item":{
                "type":"function_call",
                "call_id":"codex-router-floor-call",
                "name":"shell_command",
                "arguments":r#"{"command":"printf codex-router-tool-ok","timeout_ms":1000}"#
            }
        }),
    ];
    for event in events {
        socket
            .send(Message::Text(event.to_string().into()))
            .map_err(|error| format!("floor upstream tool-call event failed: {error}"))?;
    }
    Ok(())
}

fn send_first_turn_completed(socket: &mut WebSocket<TcpStream>) -> Result<(), String> {
    let event = serde_json::json!({
        "type":"response.completed",
        "response":{
            "id":"resp-floor-first",
            "usage":{
                "input_tokens":0,
                "input_tokens_details":null,
                "output_tokens":0,
                "output_tokens_details":null,
                "total_tokens":0
            }
        }
    });
    socket
        .send(Message::Text(event.to_string().into()))
        .map_err(|error| format!("floor upstream first completion failed: {error}"))
}

fn expect_router_reconnect(socket: &mut WebSocket<TcpStream>) -> Result<(), String> {
    loop {
        match socket.read() {
            Ok(Message::Close(_))
            | Err(tungstenite::Error::ConnectionClosed)
            | Err(tungstenite::Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)) => {
                return Ok(());
            }
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(_) => {
                return Err("Router forwarded a new create before the floor reconnect".to_owned());
            }
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(error) => {
                return Err(format!(
                    "Router did not reconnect the floor socket: {error}"
                ));
            }
        }
    }
}
