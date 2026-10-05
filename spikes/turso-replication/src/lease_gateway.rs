//! One-project PoC: central durable lease, whole-request fence, serialized handover.
use anyhow::{Result, ensure};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{OriginalUri, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct LeaseClaim {
    pub holder: String,
    pub epoch: i64,
    pub writable: bool,
}
#[derive(Clone)]
struct GatewayState {
    control: Arc<Mutex<turso::Connection>>,
    tokens: Arc<std::collections::HashMap<String, LeaseClaim>>,
    upstream: String,
    client: reqwest::Client,
    clock: Arc<AtomicI64>,
    forwarded_mutations: Arc<AtomicU64>,
    denied_mutations: Arc<AtomicU64>,
}
pub struct LeaseGateway {
    state: GatewayState,
    task: tokio::task::JoinHandle<()>,
    pub url: String,
}

impl LeaseGateway {
    pub async fn start(path: &Path, upstream: String) -> Result<Self> {
        let database = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await?;
        let control = database.connect()?;
        control.execute_batch("CREATE TABLE project_lease(project TEXT PRIMARY KEY, holder TEXT NOT NULL, epoch INTEGER NOT NULL, expires_at INTEGER NOT NULL); INSERT INTO project_lease VALUES('project','A',1,100);").await?;
        let mut tokens = std::collections::HashMap::new();
        for (token, holder, epoch, writable) in [
            ("a-epoch-1", "A", 1, true),
            ("b-epoch-2", "B", 2, true),
            ("a-epoch-3", "A", 3, true),
            ("reader-a", "A", 0, false),
            ("reader-b", "B", 0, false),
        ] {
            tokens.insert(
                token.to_string(),
                LeaseClaim {
                    holder: holder.into(),
                    epoch,
                    writable,
                },
            );
        }
        let state = GatewayState {
            control: Arc::new(Mutex::new(control)),
            tokens: Arc::new(tokens),
            upstream,
            client: reqwest::Client::new(),
            clock: Arc::new(AtomicI64::new(0)),
            forwarded_mutations: Arc::new(AtomicU64::new(0)),
            denied_mutations: Arc::new(AtomicU64::new(0)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let app = Router::new()
            .route("/v2/pipeline", post(pipeline))
            .route("/pull-updates", post(pull))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                eprintln!("gateway serve error {error}");
            }
        });
        println!(
            "GATEWAY {url} central lease={} backend={}",
            path.display(),
            state.upstream
        );
        Ok(Self { state, task, url })
    }
    pub fn expire_clock(&self) {
        self.state.clock.store(101, Ordering::SeqCst);
        println!("LEASE test clock advanced to 101 (expiry=100)");
    }
    pub async fn transfer(&self, clean: bool) -> Result<()> {
        let connection = self.state.control.lock().await;
        let mut rows = connection
            .query(
                "SELECT holder,epoch,expires_at FROM project_lease WHERE project='project'",
                (),
            )
            .await?;
        let row = rows.next().await?.unwrap();
        let holder: String = row.get(0)?;
        let epoch: i64 = row.get(1)?;
        let expires_at: i64 = row.get(2)?;
        ensure!(holder == "A" && epoch == 1, "unexpected existing lease");
        ensure!(
            clean || self.state.clock.load(Ordering::SeqCst) >= expires_at,
            "unclean transfer before expiry"
        );
        connection.execute("UPDATE project_lease SET holder='B',epoch=2,expires_at=300 WHERE project='project' AND holder='A' AND epoch=1", ()).await?;
        println!(
            "LEASE {} transfer A/1 -> B/2 persisted",
            if clean { "drained release" } else { "expired" }
        );
        Ok(())
    }
    pub fn counters(&self) -> (u64, u64) {
        (
            self.state.forwarded_mutations.load(Ordering::SeqCst),
            self.state.denied_mutations.load(Ordering::SeqCst),
        )
    }
    pub async fn admit_local(&self, claim: &LeaseClaim) -> Result<()> {
        let control = self.state.control.lock().await;
        validate_claim(&self.state, &control, claim).await
    }
}
impl Drop for LeaseGateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn validate_claim(
    state: &GatewayState,
    control: &turso::Connection,
    claim: &LeaseClaim,
) -> Result<()> {
    ensure!(claim.writable, "read-only capability");
    let row = control
        .query(
            "SELECT holder,epoch,expires_at FROM project_lease WHERE project='project'",
            (),
        )
        .await?
        .next()
        .await?
        .unwrap();
    let holder: String = row.get(0)?;
    let epoch: i64 = row.get(1)?;
    let expires_at: i64 = row.get(2)?;
    ensure!(
        claim.holder == holder
            && claim.epoch == epoch
            && state.clock.load(Ordering::SeqCst) < expires_at,
        "stale lease holder={}/{} current={holder}/{epoch}",
        claim.holder,
        claim.epoch
    );
    Ok(())
}
fn response(status: StatusCode, body: impl Into<Body>) -> Response {
    Response::builder()
        .status(status)
        .body(body.into())
        .unwrap()
}
fn token_claim(state: &GatewayState, headers: &HeaderMap) -> Option<LeaseClaim> {
    let token = headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    state.tokens.get(token).cloned()
}
// Only exact SDK SELECT-only request shapes can bypass writer admission.
fn readonly_pipeline(body: &Bytes) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let Some(requests) = value["requests"].as_array() else {
        return false;
    };
    !requests.is_empty()
        && requests.iter().all(|request| {
            request["type"] == "execute"
                && request["stmt"]["sql"].as_str().is_some_and(|sql| {
                    let sql = sql.trim();
                    sql.to_ascii_uppercase().starts_with("SELECT ") && !sql.contains(';')
                })
        })
}
async fn pipeline(
    State(state): State<GatewayState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(claim) = token_claim(&state, &headers) else {
        return response(StatusCode::UNAUTHORIZED, "unknown PoC capability");
    };
    if readonly_pipeline(&body) {
        return forward(&state, uri.path(), body).await;
    }
    let control = state.control.lock().await;
    if let Err(error) = validate_claim(&state, &control, &claim).await {
        state.denied_mutations.fetch_add(1, Ordering::SeqCst);
        println!(
            "GATEWAY REFUSED whole pipeline bytes={} reason={error}",
            body.len()
        );
        return response(StatusCode::CONFLICT, error.to_string());
    }
    state.forwarded_mutations.fetch_add(1, Ordering::SeqCst);
    let result = forward(&state, uri.path(), body).await;
    drop(control); // Transfer cannot pass an admitted in-flight push.
    result
}
async fn pull(
    State(state): State<GatewayState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if token_claim(&state, &headers).is_none() {
        return response(StatusCode::UNAUTHORIZED, "unknown PoC capability");
    }
    forward(&state, uri.path(), body).await
}
async fn forward(state: &GatewayState, path: &str, body: Bytes) -> Response {
    match state
        .client
        .post(format!("{}{path}", state.upstream))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(result) => {
            let status = result.status();
            let content_type = result.headers().get("content-type").cloned();
            match result.bytes().await {
                Ok(bytes) => {
                    let mut response = response(status, Body::from(bytes));
                    if let Some(content_type) = content_type {
                        response.headers_mut().insert("content-type", content_type);
                    }
                    response
                }
                Err(error) => response(StatusCode::BAD_GATEWAY, error.to_string()),
            }
        }
        Err(error) => response(StatusCode::BAD_GATEWAY, error.to_string()),
    }
}
