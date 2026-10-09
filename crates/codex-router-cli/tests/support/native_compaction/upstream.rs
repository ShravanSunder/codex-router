use super::fixture::SYNTHETIC_POOL_TOKEN;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    error::Error,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
    },
};

pub const OPAQUE_COMPACTION: &str = "SYNTHETIC_OPAQUE_NATIVE_V2_COMPACTION";
#[derive(Clone, Debug)]
pub struct Observation {
    pub transport: &'static str,
    pub path: String,
    pub compaction_trigger: bool,
    pub retained_compaction: bool,
    pub continuation: bool,
    pub inline_summary: bool,
    pub content_encoding: bool,
    pub parseable_json: bool,
    pub synthetic_pool_authorization: bool,
}
pub struct ResponsesFixture {
    address: SocketAddr,
    observed: Arc<Mutex<Vec<Observation>>>,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), String>>,
}
impl ResponsesFixture {
    pub async fn start() -> Result<Self, Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let observed = Arc::new(Mutex::new(Vec::new()));
        let task_observed = Arc::clone(&observed);
        let (shutdown, mut receive) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut receive => break,
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.map_err(|error| error.to_string())?;
                        let observed = Arc::clone(&task_observed);
                        connections.spawn(async move { tokio::time::timeout(Duration::from_secs(30), handle_connection(stream, observed)).await.map_err(|error|error.to_string())? });
                    },
                    result = connections.join_next(), if !connections.is_empty() => {
                        result.ok_or("missing connection result")?.map_err(|error|error.to_string())??;
                    }
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
            Ok(())
        });
        Ok(Self {
            address,
            observed,
            shutdown: Some(shutdown),
            task,
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub async fn observations(&self) -> Result<Vec<Observation>, String> {
        Ok(self
            .observed
            .lock()
            .map_err(|_| "fixture observation lock poisoned")?
            .clone())
    }
    pub async fn stop(mut self) -> Result<(), Box<dyn Error>> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task
            .await?
            .map_err(|error| -> Box<dyn Error> { error.into() })?;
        if TcpStream::connect(self.address).await.is_ok() {
            return Err("owned mock upstream listener survived cleanup".into());
        }
        eprintln!("native_compaction_cleanup mock_listener_absent=true");
        Ok(())
    }
}

#[allow(clippy::result_large_err)] // tungstenite handshake callback has a fixed large error type.
async fn handle_connection(
    mut stream: TcpStream,
    observed: Arc<Mutex<Vec<Observation>>>,
) -> Result<(), String> {
    let mut prefix = [0; 4096];
    let count = stream
        .peek(&mut prefix)
        .await
        .map_err(|error| error.to_string())?;
    if String::from_utf8_lossy(prefix.get(..count).ok_or("invalid HTTP peek length")?)
        .to_ascii_lowercase()
        .contains("upgrade: websocket")
    {
        let handshake = Arc::new(Mutex::new((String::new(), false)));
        let captured = Arc::clone(&handshake);
        let mut socket = accept_hdr_async(stream, move |request: &Request, response: Response| {
            *captured.lock().map_err(|_| {
                tokio_tungstenite::tungstenite::http::Response::new(Some(
                    "fixture handshake mutex poisoned".to_owned(),
                ))
            })? = (
                request.uri().path().to_owned(),
                request
                    .headers()
                    .get("authorization")
                    .and_then(|header| header.to_str().ok())
                    == Some(format!("Bearer {SYNTHETIC_POOL_TOKEN}").as_str()),
            );
            Ok(response)
        })
        .await
        .map_err(|error| error.to_string())?;
        while let Some(message) = socket.next().await {
            let message = message.map_err(|error| error.to_string())?;
            if let Message::Text(text) = message {
                let body: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
                let (path, auth) = handshake
                    .lock()
                    .map_err(|_| "handshake mutex poisoned")?
                    .clone();
                let request = observation("websocket", path, &body, false, auth);
                let fail = request.compaction_trigger;
                observed
                    .lock()
                    .map_err(|_| "observation mutex poisoned")?
                    .push(request);
                if fail {
                    return Ok(());
                } // Accept WS, then disconnect without response.completed.
                for event in response_events(&body) {
                    socket
                        .send(Message::Text(event.to_string().into()))
                        .await
                        .map_err(|error| error.to_string())?;
                }
            } else if message.is_close() {
                return Ok(());
            }
        }
        return Ok(());
    }
    let mut data = Vec::new();
    let header_end = loop {
        if let Some(position) = data.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break position + 4;
        }
        let mut bytes = [0; 4096];
        let count = stream
            .read(&mut bytes)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("incomplete HTTP header".into());
        }
        data.extend_from_slice(bytes.get(..count).ok_or("invalid HTTP read length")?);
        if data.len() > 65_536 {
            return Err("HTTP fixture header exceeded bound".into());
        }
    };
    let headers = String::from_utf8_lossy(
        data.get(..header_end)
            .ok_or("invalid HTTP header boundary")?,
    )
    .to_string();
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .ok_or("HTTP fixture requires content-length")?;
    if length > 1_048_576 {
        return Err("HTTP fixture body exceeded bound".into());
    }
    while data.len() < header_end + length {
        let mut bytes = [0; 4096];
        let count = stream
            .read(&mut bytes)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("incomplete HTTP body".into());
        }
        data.extend_from_slice(bytes.get(..count).ok_or("invalid HTTP read length")?);
    }
    let body: Value = serde_json::from_slice(
        data.get(header_end..header_end + length)
            .ok_or("invalid HTTP body boundary")?,
    )
    .map_err(|_| "HTTP fallback body not uncompressed JSON")?;
    let path = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("missing HTTP path")?
        .split('?')
        .next()
        .ok_or("missing HTTP path")?
        .to_owned();
    let encoding = headers
        .lines()
        .any(|line| line.to_ascii_lowercase().starts_with("content-encoding:"));
    let auth = headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(key, value)| {
            key.eq_ignore_ascii_case("authorization")
                && value.trim() == format!("Bearer {SYNTHETIC_POOL_TOKEN}")
        })
    });
    observed
        .lock()
        .map_err(|_| "observation mutex poisoned")?
        .push(observation("http", path, &body, encoding, auth));
    let events: String = response_events(&body)
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{events}",
        events.len()
    );
    stream
        .write_all(reply.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn observation(
    transport: &'static str,
    path: String,
    body: &Value,
    encoding: bool,
    auth: bool,
) -> Observation {
    let input = body.pointer("/input").unwrap_or(&Value::Null).as_array();
    let contains_type = |kind: &str| {
        input.is_some_and(|items| {
            items
                .iter()
                .any(|item| item.pointer("/type").unwrap_or(&Value::Null) == kind)
        })
    };
    let text = body.to_string();
    Observation {
        transport,
        path,
        compaction_trigger: contains_type("compaction_trigger"),
        retained_compaction: input.is_some_and(|items| {
            items.iter().any(|item| {
                item.pointer("/type").unwrap_or(&Value::Null) == "compaction"
                    && item.pointer("/encrypted_content").unwrap_or(&Value::Null)
                        == OPAQUE_COMPACTION
            })
        }),
        continuation: text.contains("CONTINUE_NATIVE_COMPACTION"),
        inline_summary: text.contains("CONTEXT CHECKPOINT COMPACTION"),
        content_encoding: encoding,
        parseable_json: true,
        synthetic_pool_authorization: auth,
    }
}

fn response_events(body: &Value) -> Vec<Value> {
    let compact = body
        .pointer("/input")
        .unwrap_or(&Value::Null)
        .as_array()
        .is_some_and(|input| {
            input
                .iter()
                .any(|item| item.pointer("/type").unwrap_or(&Value::Null) == "compaction_trigger")
        });
    let warmup = body.pointer("/generate").unwrap_or(&Value::Null) == false;
    let item = if compact {
        json!({"type":"compaction","encrypted_content":OPAQUE_COMPACTION})
    } else {
        json!({"type":"message","id":"msg_fixture","role":"assistant","content":[{"type":"output_text","text":"NATIVE_COMPACTION_FIXTURE_OK"}]})
    };
    let id = if compact {
        "resp_fixture_compaction"
    } else {
        "resp_fixture_sampling"
    };
    let mut events = vec![json!({"type":"response.created","response":{"id":id}})];
    if !warmup {
        events.push(json!({"type":"response.output_item.done","output_index":0,"item":item}));
    }
    events.push(json!({"type":"response.completed","response":{"id":id,"status":"completed","output":if warmup {vec![]} else {vec![item]},"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}));
    events
}
