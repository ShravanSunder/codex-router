//! A stand-in collaboration API for CLI tests that need answers the real Router never
//! gives: malformed results, lost responses, or a fixed rejection.
//!
//! It publishes a version 3 manifest beside `control.sock` and answers each MCP
//! `tools/call` (one HTTP/1.1 request per connection, as the CLI's client sends them) with
//! the reply the test scripted. Connections that carry no request, such as the client's
//! reachability probe, are skipped.
#![allow(dead_code)]

use collaboration_service::ManifestPublication;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    task::JoinHandle,
};

pub type FakeResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

/// How the stand-in answers one tool call.
#[derive(Clone, Debug)]
pub enum FakeReply {
    /// A successful tool result whose structured content is this value, valid or not.
    Result(Value),
    /// The rejection a Router publishes: `{"code", "message", "data"}`.
    Error(Value),
    /// A stored message's receipt, presented as the Router presents one: a held or accepted
    /// delivery succeeds; a refused, rejected or unknown delivery is a tool error that still
    /// carries the receipt.
    Receipt(Value),
    /// Reads the call, then closes the connection without answering.
    Disconnect,
}

/// One tool call the CLI made: `{"tool": <name>, "arguments": <object>}`.
pub type RecordedCall = Value;

pub struct FakeCollaborationApi {
    directory: tempfile::TempDir,
    listener: Option<UnixListener>,
    _publication: ManifestPublication,
}

impl FakeCollaborationApi {
    pub fn new(service_id: &str, service_epoch: &str) -> FakeResult<Self> {
        let directory = tempfile::tempdir_in("/tmp")?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        let listener = UnixListener::bind(directory.path().join("control.sock"))?;
        let manifest = serde_json::from_value(manifest_json(service_id, service_epoch))?;
        let publication = ManifestPublication::publish(directory.path(), &manifest)?;
        Ok(Self {
            directory,
            listener: Some(listener),
            _publication: publication,
        })
    }

    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    /// Answers the CLI's tool calls in order with `replies`, one call per reply, and
    /// returns the calls it received.
    pub fn serve(&mut self, replies: Vec<FakeReply>) -> JoinHandle<FakeResult<Vec<RecordedCall>>> {
        let listener = self.listener.take();
        tokio::spawn(async move {
            let listener = listener.ok_or("the stand-in API serves once")?;
            let mut calls = Vec::new();
            for reply in replies {
                calls.push(answer_next_call(&listener, reply).await?);
            }
            Ok(calls)
        })
    }

    /// Like `serve`, then keeps listening for `quiet` and reports any further call, such
    /// as a replay the CLI must never make.
    pub fn serve_then_watch(
        &mut self,
        replies: Vec<FakeReply>,
        quiet: Duration,
    ) -> JoinHandle<FakeResult<WatchedCalls>> {
        let listener = self.listener.take();
        tokio::spawn(async move {
            let listener = listener.ok_or("the stand-in API serves once")?;
            let mut calls = Vec::new();
            for reply in replies {
                calls.push(answer_next_call(&listener, reply).await?);
            }
            let further_call = match tokio::time::timeout(quiet, next_request(&listener)).await {
                Err(_) => None,
                Ok(Ok((_, _, call))) => Some(call),
                Ok(Err(error)) => return Err(error),
            };
            Ok(WatchedCalls {
                calls,
                further_call,
            })
        })
    }
}

/// The calls answered, and any call that arrived afterwards.
pub struct WatchedCalls {
    pub calls: Vec<RecordedCall>,
    pub further_call: Option<RecordedCall>,
}

fn tool_result(id: &Value, presentation: (Value, bool)) -> Value {
    let (structured, is_error) = presentation;
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {"content": [], "structuredContent": structured, "isError": is_error}
    })
}

/// The Router's message receipt presentation: `(structured content, is error)`.
fn receipt_presentation(mut receipt: Value) -> (Value, bool) {
    if receipt.get("deliveryState").and_then(Value::as_str) == Some("held") {
        return (receipt, false);
    }
    let outcome = receipt
        .pointer("/receipt/outcome")
        .cloned()
        .unwrap_or(Value::Null);
    let (kind, message, effect) = match outcome.get("kind").and_then(Value::as_str) {
        Some("notSubmitted") => (
            "notSubmitted",
            outcome.get("reason").cloned().unwrap_or(Value::Null),
            "none",
        ),
        Some("rejected") => (
            "rejected",
            outcome
                .get("detail")
                .filter(|detail| !detail.is_null())
                .cloned()
                .unwrap_or_else(|| json!("Delivery was rejected")),
            "none",
        ),
        Some("unknown") => (
            "outcomeUnknown",
            json!("Delivery acceptance is unknown"),
            "unknown",
        ),
        _ => return (receipt, false),
    };
    if let Some(fields) = receipt.as_object_mut() {
        fields.insert("mcpResult".to_owned(), json!("error"));
        fields.insert("kind".to_owned(), json!(kind));
        fields.insert("message".to_owned(), message);
        fields.insert("effect".to_owned(), json!(effect));
    }
    (receipt, true)
}

pub fn manifest_json(service_id: &str, service_epoch: &str) -> Value {
    json!({
        "version": 3,
        "serviceId": service_id,
        "serviceEpoch": service_epoch,
        "machineLabel": "fixture-host",
        "serviceVersion": env!("CARGO_PKG_VERSION"),
        "api": {"transport": "streamableHttpUnix", "path": "control.sock"},
        "mcp": {"transport": "streamableHttp", "url": "http://127.0.0.1:0/mcp"}
    })
}

async fn answer_next_call(listener: &UnixListener, reply: FakeReply) -> FakeResult<RecordedCall> {
    let (mut stream, request, call) = next_request(listener).await?;
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let body = match reply {
        FakeReply::Disconnect => {
            stream.shutdown().await?;
            return Ok(call);
        }
        FakeReply::Result(structured) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"content": [], "structuredContent": structured, "isError": false}
        }),
        FakeReply::Receipt(receipt) => tool_result(&id, receipt_presentation(receipt)),
        FakeReply::Error(error) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [],
                "structuredContent": {
                    "mcpResult": "error",
                    "kind": "rejected",
                    "message": error.get("message").cloned().unwrap_or(Value::Null),
                    "code": error.get("code").cloned().unwrap_or(Value::Null),
                    "data": error.get("data").cloned().unwrap_or(Value::Null),
                },
                "isError": true
            }
        }),
    };
    let body = body.to_string();
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(call)
}

/// The next connection that carries a request, with the JSON-RPC message and the call.
async fn next_request(listener: &UnixListener) -> FakeResult<(UnixStream, Value, RecordedCall)> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let Some(body) = read_http_body(&mut stream).await? else {
            continue;
        };
        let request: Value = serde_json::from_slice(&body)?;
        if request.get("method").and_then(Value::as_str) != Some("tools/call") {
            return Err(format!("the CLI sent a non-tool request: {request}").into());
        }
        let call = json!({
            "tool": request.pointer("/params/name").cloned().unwrap_or(Value::Null),
            "arguments": request.pointer("/params/arguments").cloned().unwrap_or(Value::Null),
        });
        return Ok((stream, request, call));
    }
}

/// The request body, or `None` when the peer closed without sending a request.
async fn read_http_body(stream: &mut UnixStream) -> FakeResult<Option<Vec<u8>>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    let head_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return if buffer.is_empty() {
                Ok(None)
            } else {
                Err("the request ended inside its headers".into())
            };
        }
        buffer.extend_from_slice(chunk.get(..count).ok_or("read past the buffer")?);
    };
    let head = String::from_utf8_lossy(buffer.get(..head_end).ok_or("header bounds")?).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .ok_or("the request has no content length")?;
    let mut body = buffer.get(head_end..).ok_or("body bounds")?.to_vec();
    while body.len() < length {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Err("the request ended inside its body".into());
        }
        body.extend_from_slice(chunk.get(..count).ok_or("read past the buffer")?);
    }
    Ok(Some(body))
}
