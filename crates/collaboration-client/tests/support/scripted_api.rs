//! A scripted collaboration API on a service socket, for cases a real Router cannot produce.
//!
//! The client's own handling of malformed results, lost responses and published rejections is
//! proved against this endpoint: each `tools/call` the client makes surfaces as one
//! [`ScriptedCall`] the test answers, fails, or abandons. A version 3 manifest names the socket,
//! so the client connects exactly as it does to a Host.
// Each test crate that includes this module uses only part of it.
#![allow(dead_code)]
use serde_json::{Value, json};
use std::{io, path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

pub const SCRIPTED_SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
pub const SCRIPTED_SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";

/// How long a test waits for the client's next call before failing.
const CALL_WAIT: Duration = Duration::from_secs(5);
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// The served socket and its manifest; dropping it stops serving.
pub struct ScriptedApi {
    directory: tempfile::TempDir,
    accept: JoinHandle<()>,
}

/// The calls the client made, in arrival order.
pub struct ScriptedCalls {
    receiver: mpsc::UnboundedReceiver<ScriptedCall>,
}

/// One `tools/call` the client made, waiting for the test's answer.
pub struct ScriptedCall {
    pub tool: String,
    pub arguments: Value,
    answer: oneshot::Sender<ScriptedAnswer>,
}

enum ScriptedAnswer {
    Result(Value),
    ToolError(Value),
    JsonRpcError(Value),
}

impl ScriptedApi {
    pub async fn start() -> io::Result<(Self, ScriptedCalls)> {
        let directory = tempfile::tempdir()?;
        let listener = UnixListener::bind(directory.path().join("control.sock"))?;
        let manifest = json!({
            "version": 3,
            "serviceId": SCRIPTED_SERVICE_ID,
            "serviceEpoch": SCRIPTED_SERVICE_EPOCH,
            "machineLabel": "fixture-host",
            "serviceVersion": env!("CARGO_PKG_VERSION"),
            "api": {"transport": "streamableHttpUnix", "path": "control.sock"},
            "mcp": {"transport": "streamableHttp", "url": "http://127.0.0.1:0/mcp"}
        });
        std::fs::write(
            directory.path().join("service.json"),
            serde_json::to_vec(&manifest).map_err(io::Error::other)?,
        )?;
        let (sender, receiver) = mpsc::unbounded_channel();
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(answer_connection(stream, sender.clone()));
            }
        });
        Ok((Self { directory, accept }, ScriptedCalls { receiver }))
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
}

impl Drop for ScriptedApi {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl ScriptedCalls {
    /// The client's next call; fails when none arrives within a few seconds.
    pub async fn next(&mut self) -> io::Result<ScriptedCall> {
        tokio::time::timeout(CALL_WAIT, self.receiver.recv())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no call arrived"))?
            .ok_or_else(|| io::Error::other("scripted API stopped"))
    }

    /// True when no call arrives within `window`.
    pub async fn none_within(&mut self, window: Duration) -> bool {
        tokio::time::timeout(window, self.receiver.recv())
            .await
            .is_err()
    }
}

impl ScriptedCall {
    /// Answers the call with a successful structured tool result.
    pub fn succeed(self, result: Value) {
        let _ignored = self.answer.send(ScriptedAnswer::Result(result));
    }

    /// Answers the call with a structured tool error, as the API publishes failures.
    pub fn fail(self, failure: Value) {
        let _ignored = self.answer.send(ScriptedAnswer::ToolError(failure));
    }

    /// Answers the call with a JSON-RPC error.
    pub fn reject(self, error: Value) {
        let _ignored = self.answer.send(ScriptedAnswer::JsonRpcError(error));
    }

    /// Closes the call's connection without any answer: the response is lost.
    pub fn lose_response(self) {
        drop(self.answer);
    }
}

async fn answer_connection(mut stream: UnixStream, calls: mpsc::UnboundedSender<ScriptedCall>) {
    // A connection that sends nothing is the client's reachability probe.
    let Ok(Some(body)) = read_request_body(&mut stream).await else {
        return;
    };
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        return;
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let (answer, answered) = oneshot::channel();
    let call = ScriptedCall {
        tool: request
            .pointer("/params/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        arguments: request
            .pointer("/params/arguments")
            .cloned()
            .unwrap_or(Value::Null),
        answer,
    };
    if calls.send(call).is_err() {
        return;
    }
    let Ok(answer) = answered.await else {
        return;
    };
    let message = match answer {
        ScriptedAnswer::Result(result) => json!({"jsonrpc":"2.0","id":id,"result":{
            "content":[{"type":"text","text":result.to_string()}],
            "structuredContent":result,
            "isError":false
        }}),
        ScriptedAnswer::ToolError(failure) => json!({"jsonrpc":"2.0","id":id,"result":{
            "content":[{"type":"text","text":failure.to_string()}],
            "structuredContent":failure,
            "isError":true
        }}),
        ScriptedAnswer::JsonRpcError(error) => {
            json!({"jsonrpc":"2.0","id":id,"error":error})
        }
    };
    let body = message.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ignored = stream.write_all(response.as_bytes()).await;
    let _ignored = stream.shutdown().await;
}

/// Reads one HTTP/1.1 request and answers its body; `None` when the peer sent nothing.
async fn read_request_body(stream: &mut UnixStream) -> io::Result<Option<Vec<u8>>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    let header_end = loop {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Ok(None);
        }
        buffer.extend(chunk.iter().take(count));
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        if buffer.len() > MAX_REQUEST_BYTES {
            return Err(io::Error::other("request headers too large"));
        }
    };
    let headers = String::from_utf8_lossy(buffer.get(..header_end).unwrap_or_default());
    let length = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .ok_or_else(|| io::Error::other("request without content length"))?;
    if length > MAX_REQUEST_BYTES {
        return Err(io::Error::other("request body too large"));
    }
    let mut body = buffer.split_off(header_end);
    while body.len() < length {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        body.extend(chunk.iter().take(count));
    }
    body.truncate(length);
    Ok(Some(body))
}
