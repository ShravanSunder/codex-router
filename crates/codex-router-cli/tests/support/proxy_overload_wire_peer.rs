//! Provider WebSocket peer and literal handshake/frame observation for overload tests.
use futures_util::SinkExt;
use serde_json::Value;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::{Duration as TokioDuration, timeout};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, handshake::server::Request},
};
const UPSTREAM_ACCEPT_TIMEOUT: TokioDuration = TokioDuration::from_secs(5);

pub(crate) type ProviderWebSocket = WebSocketStream<TcpStream>;
pub(crate) type TestLocalWebSocket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderHandshakeObservation {
    pub(crate) request_path: String,
    pub(crate) selected_account_id: Option<String>,
    pub(crate) thread_id_values: Vec<String>,
}

pub(crate) struct ProviderConnection {
    pub(crate) websocket: ProviderWebSocket,
    pub(crate) handshake: ProviderHandshakeObservation,
    pub(super) explicit_response_create_count: usize,
}

impl ProviderConnection {
    pub(crate) async fn receive_client_frame(&mut self) -> Result<Message, String> {
        let message = timeout(
            UPSTREAM_ACCEPT_TIMEOUT,
            futures_util::StreamExt::next(&mut self.websocket),
        )
        .await
        .map_err(|_| "wait for explicit client frame at fake upstream".to_owned())?
        .ok_or_else(|| "Router upstream socket ended before the expected client frame".to_owned())?
        .map_err(|error| format!("read explicit client frame at fake upstream: {error}"))?;
        self.note_response_create(&message);
        Ok(message)
    }

    pub(crate) async fn send_provider_frame(
        &mut self,
        payload: &'static str,
    ) -> Result<(), String> {
        timeout(
            UPSTREAM_ACCEPT_TIMEOUT,
            self.websocket.send(Message::Text(payload.into())),
        )
        .await
        .map_err(|_| "send fake provider frame to Router within 5 seconds".to_owned())?
        .map_err(|error| format!("send fake provider frame to Router: {error}"))
    }

    pub(crate) async fn receive_unrequested_frame(
        &mut self,
        observation_window: Duration,
    ) -> Result<Option<Message>, String> {
        match timeout(
            observation_window,
            futures_util::StreamExt::next(&mut self.websocket),
        )
        .await
        {
            Ok(Some(Ok(message))) => {
                self.note_response_create(&message);
                Ok(Some(message))
            }
            Ok(Some(Err(error))) => Err(format!("read same-socket continuation probe: {error}")),
            Ok(None) => Ok(Some(Message::Close(None))),
            Err(_) => Ok(None),
        }
    }

    pub(crate) const fn explicit_response_create_count(&self) -> usize {
        self.explicit_response_create_count
    }

    fn note_response_create(&mut self, message: &Message) {
        let Message::Text(text) = message else {
            return;
        };
        let is_response_create = serde_json::from_str::<Value>(text.as_str())
            .ok()
            .and_then(|value| value.get("type").and_then(Value::as_str).map(str::to_owned))
            .is_some_and(|message_type| message_type == "response.create");
        if is_response_create {
            self.explicit_response_create_count += 1;
        }
    }
}

pub(crate) async fn connect_local_client(
    router_port: u16,
    thread_id_values: &[String],
) -> Result<WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, String> {
    let mut request = format!("ws://127.0.0.1:{router_port}/v1/responses")
        .into_client_request()
        .map_err(|error| format!("build local Router WebSocket request: {error}"))?;
    for thread_id in thread_id_values {
        let header_value =
            tokio_tungstenite::tungstenite::http::HeaderValue::from_bytes(thread_id.as_bytes())
                .map_err(|error| format!("build fixture thread-id header: {error}"))?;
        request.headers_mut().append("thread-id", header_value);
    }

    timeout(
        UPSTREAM_ACCEPT_TIMEOUT,
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| "connect explicit WebSocket client to Router within 5 seconds".to_owned())?
    .map(|(websocket, _)| websocket)
    .map_err(|error| format!("connect explicit WebSocket client to Router: {error}"))
}

pub(crate) async fn send_explicit_response_create(
    websocket: &mut WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    sequence: usize,
) -> Result<String, String> {
    let payload = format!(r#"{{"type":"response.create","input":"fixture-{sequence}"}}"#);
    timeout(
        UPSTREAM_ACCEPT_TIMEOUT,
        websocket.send(Message::Text(payload.clone().into())),
    )
    .await
    .map_err(|_| "send explicit client response.create within 5 seconds".to_owned())?
    .map_err(|error| format!("send explicit client response.create: {error}"))?;
    Ok(payload)
}

pub(crate) async fn receive_client_frame(
    websocket: &mut WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
) -> Result<Message, String> {
    timeout(
        UPSTREAM_ACCEPT_TIMEOUT,
        futures_util::StreamExt::next(websocket),
    )
    .await
    .map_err(|_| "wait for explicit client WebSocket frame within 5 seconds".to_owned())?
    .ok_or_else(|| "local Router WebSocket ended before the expected frame".to_owned())?
    .map_err(|error| format!("read explicit client WebSocket frame: {error}"))
}

pub(super) fn capture_upstream_handshake(request: &Request) -> ProviderHandshakeObservation {
    let thread_id_values = request
        .headers()
        .get_all("thread-id")
        .iter()
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .collect();
    let selected_account_id = request
        .headers()
        .get("chatgpt-account-id")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
    ProviderHandshakeObservation {
        request_path: request.uri().path().to_owned(),
        selected_account_id,
        thread_id_values,
    }
}
