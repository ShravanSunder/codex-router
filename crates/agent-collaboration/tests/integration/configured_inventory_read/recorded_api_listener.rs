//! Test-only wire observer forwards every read to the genuine API unchanged.
//! It records method names, never manufactures a tool response or provider rejection.
use axum::{Router, body::Body, extract::State, response::Response, routing::post};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

struct RecorderState {
    api_url: String,
    client: reqwest::Client,
    methods: Mutex<Vec<String>>,
}

pub(super) struct RecordedApiListener {
    pub(super) url: String,
    state: Arc<RecorderState>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl RecordedApiListener {
    pub(super) async fn start(api_url: String, shutdown: CancellationToken) -> Self {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(RecorderState {
            api_url,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            methods: Mutex::new(Vec::new()),
        });
        let router = Router::new()
            .route("/mcp", post(record_read))
            .with_state(Arc::clone(&state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
        Self {
            url: format!("http://{address}/mcp"),
            state,
            task,
        }
    }

    pub(super) fn call_count(&self, method: &str) -> usize {
        self.state
            .methods
            .lock()
            .unwrap()
            .iter()
            .filter(|observed| observed.as_str() == method)
            .count()
    }

    pub(super) async fn finish(self) {
        tokio::time::timeout(Duration::from_secs(2), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

async fn record_read(
    State(state): State<Arc<RecorderState>>,
    request: axum::extract::Request,
) -> Response {
    let bytes = axum::body::to_bytes(
        request.into_body(),
        collaboration_client::protocol::MAX_CONTROL_FRAME_BYTES,
    )
    .await
    .unwrap();
    let message: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    if let Some(method) = message
        .pointer("/params/name")
        .and_then(serde_json::Value::as_str)
    {
        state.methods.lock().unwrap().push(method.to_owned());
    }
    let response = state
        .client
        .post(&state.api_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header("mcp-protocol-version", "2025-11-25")
        .body(bytes)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .clone();
    let body = response.bytes().await.unwrap();
    Response::builder()
        .status(status)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap()
}
