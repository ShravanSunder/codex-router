//! Protocol-fault peers stand in only for replies the real server cannot intentionally emit.
use super::*;
use crate::presentation::session_picker::{
    PickerSourceContext, SourceInventoryRejection, SourceInventoryRequest,
};
use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
const EPOCH: &str = "00000000-0000-4000-8000-000000000002";

#[derive(Clone, Copy)]
pub(crate) enum ReplyScenario {
    ChangedEpoch,
    WrongRequestId,
    WrongPageEndpoint,
    RepeatedCursor,
    ProviderRows,
    HoldPage,
    PageBudget { complete: bool },
    RowBudget(usize),
    SparsePages,
    ProviderSkipped,
}

struct PeerState {
    scenario: ReplyScenario,
    calls: Mutex<Vec<(String, Value)>>,
    native_started: tokio::sync::Semaphore,
    release_page: tokio::sync::Semaphore,
    held_settled: tokio::sync::Semaphore,
}

pub(crate) struct ReadPeer {
    url: String,
    state: Arc<PeerState>,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl ReadPeer {
    pub(crate) async fn start(scenario: ReplyScenario) -> Self {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(PeerState {
            scenario,
            calls: Mutex::new(Vec::new()),
            native_started: tokio::sync::Semaphore::new(0),
            release_page: tokio::sync::Semaphore::new(0),
            held_settled: tokio::sync::Semaphore::new(0),
        });
        let router = Router::new()
            .route("/mcp", post(reply))
            .with_state(state.clone());
        let shutdown = CancellationToken::new();
        let cancel = shutdown.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(cancel.cancelled_owned())
                .await
        });
        Self {
            url: format!("http://{address}/mcp"),
            state,
            shutdown,
            task,
        }
    }

    pub(crate) fn request(&self) -> SourceInventoryRequest {
        let registry = router_connection_registry::RouterConnectionRegistry::parse(&json!({"version":1,"routers":[{
            "name":"Source peer","connection":{"kind":"remote","serviceId":SERVICE,"mcpUrl":self.url}
        }]}).to_string()).unwrap();
        SourceInventoryRequest {
            source_context: PickerSourceContext::ConfiguredHosted(registry.routers[0].clone()),
            query: SessionsPickerDataQuery {
                root: SessionsPickerRoot::Any,
                provider: SessionsProvider::Any,
                source: SessionsSource::All,
                sort: SessionsSort::Updated,
                search: String::new(),
                include_empty_sessions: false,
            },
            request_generation: 9,
        }
    }

    pub(crate) fn loader(&self) -> SessionsPickerRecordLoader {
        session_picker_record_loader(
            CliContext::new(Vec::new()),
            discover_repository_identity(std::path::Path::new("/tmp")),
            None,
        )
    }

    pub(crate) async fn wait_for_held_page(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.state.native_started.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }

    pub(crate) async fn release_held_page(&self) {
        self.state.release_page.add_permits(1);
        tokio::time::timeout(Duration::from_secs(2), self.state.held_settled.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }

    pub(crate) fn inventory_calls(&self) -> usize {
        self.state
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _)| method == "endpoints_list")
            .count()
    }

    pub(crate) async fn finish(self) {
        self.state.release_page.add_permits(1);
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

struct HeldReplyGuard(Arc<PeerState>);
impl Drop for HeldReplyGuard {
    fn drop(&mut self) {
        self.0.held_settled.add_permits(1);
    }
}

async fn reply(State(state): State<Arc<PeerState>>, Json(request): Json<Value>) -> Json<Value> {
    let method = request
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();
    let arguments = request.pointer("/params/arguments").unwrap().clone();
    let (inventory_calls, native_calls) = {
        let mut calls = state.calls.lock().unwrap();
        calls.push((method.clone(), arguments));
        (
            calls
                .iter()
                .filter(|(name, _)| name == "endpoints_list")
                .count(),
            calls
                .iter()
                .filter(|(name, _)| name == "sessions_list")
                .count(),
        )
    };
    let native_endpoint = json!({"serviceId":SERVICE,"endpointId":"codex-local"});
    let provider_endpoint = json!({"serviceId":SERVICE,"endpointId":"claude-local"});
    let result = match method.as_str() {
        "endpoints_list" => {
            let mut endpoints = vec![
                json!({"endpoint":native_endpoint,"label":"Native","availability":{"state":"unprobed"},
                "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]}),
            ];
            if matches!(
                state.scenario,
                ReplyScenario::ProviderRows | ReplyScenario::ProviderSkipped
            ) {
                endpoints.push(json!({"endpoint":provider_endpoint,"label":"Source provider","availability":{"state":"unprobed"},
                    "channels":[{"kind":"externalProvider","transport":"stdioAcp","bindingId":"00000000-0000-4000-8000-000000000003","bindingGeneration":1,
                        "runtime":{"provider":"claudeCode","runtimeName":"claude"},"capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]}));
            }
            let epoch =
                if matches!(state.scenario, ReplyScenario::ChangedEpoch) && inventory_calls > 1 {
                    "00000000-0000-4000-8000-000000000004"
                } else {
                    EPOCH
                };
            json!({"serviceEpoch":epoch,"sequence":1,"endpoints":endpoints})
        }
        "sessions_list" => {
            if matches!(state.scenario, ReplyScenario::HoldPage) && native_calls == 1 {
                let _settled = HeldReplyGuard(Arc::clone(&state));
                state.native_started.add_permits(1);
                state.release_page.acquire().await.unwrap().forget();
            }
            let endpoint = if matches!(state.scenario, ReplyScenario::WrongPageEndpoint) {
                json!({"serviceId":SERVICE,"endpointId":"wrong-endpoint"})
            } else {
                native_endpoint.clone()
            };
            let cursor = match state.scenario {
                ReplyScenario::RepeatedCursor => Some("repeated".to_owned()),
                ReplyScenario::PageBudget { complete } if !complete || native_calls < 65 => {
                    Some(format!("opaque +/=?☀ {native_calls}"))
                }
                ReplyScenario::RowBudget(total) if native_calls * 100 < total => {
                    Some(format!("opaque +/=?☀ {native_calls}"))
                }
                ReplyScenario::SparsePages if native_calls < 3 => {
                    Some(format!("opaque +/=?☀ {native_calls}"))
                }
                _ => None,
            };
            let sessions = if let ReplyScenario::RowBudget(total) = state.scenario {
                let first = (native_calls - 1) * 100;
                (first..total.min(first + 100))
                    .map(|index| native_row(&native_endpoint, index))
                    .collect::<Vec<_>>()
            } else if matches!(state.scenario, ReplyScenario::SparsePages) && native_calls == 3 {
                vec![native_row(&native_endpoint, 0)]
            } else {
                Vec::new()
            };
            json!({"endpoint":endpoint,"observedAt":"2026-10-11T00:00:00Z","sessions":sessions,"generation":null,"nextCursor":cursor})
        }
        "provider_sessions_list" => {
            json!({"skippedRecords":if matches!(state.scenario,ReplyScenario::ProviderSkipped){1}else{0},"endpoint":provider_endpoint,"observedAt":"2026-10-11T00:00:00Z","nextCursor":null,
            "sessions":[{"target":{"endpoint":provider_endpoint,"sessionId":"provider-only-id"},"workingDirectory":"/source/provider",
                "updatedAt":2000,"state":"idle","approver":{"endpoint":native_endpoint,"sessionId":"fixture-owner"},
                "createdBy":{"endpoint":native_endpoint,"sessionId":"fixture-owner"}}]})
        }
        _ => panic!("unexpected read method"),
    };
    let id = if matches!(state.scenario, ReplyScenario::WrongRequestId) {
        json!(99)
    } else {
        request["id"].clone()
    };
    Json(
        json!({"jsonrpc":"2.0","id":id,"result":{"content":[],"structuredContent":result,"isError":false}}),
    )
}

#[tokio::test]
async fn protocol_faults_and_epoch_change_reject_without_empty_success_or_replay() {
    for (scenario, expected, native_calls) in [
        (
            ReplyScenario::ChangedEpoch,
            SourceInventoryRejection::StaleSnapshot,
            1,
        ),
        (
            ReplyScenario::WrongRequestId,
            SourceInventoryRejection::InvalidInventory,
            0,
        ),
        (
            ReplyScenario::WrongPageEndpoint,
            SourceInventoryRejection::InvalidInventory,
            1,
        ),
        (
            ReplyScenario::RepeatedCursor,
            SourceInventoryRejection::InvalidInventory,
            2,
        ),
        (
            ReplyScenario::PageBudget { complete: false },
            SourceInventoryRejection::InvalidInventory,
            65,
        ),
        (
            ReplyScenario::RowBudget(4097),
            SourceInventoryRejection::InvalidInventory,
            41,
        ),
        (
            ReplyScenario::ProviderSkipped,
            SourceInventoryRejection::InvalidInventory,
            0,
        ),
    ] {
        let peer = ReadPeer::start(scenario).await;
        let request = peer.request();
        let result = peer.loader()(request.clone()).await;
        assert_eq!(result.request(), &request);
        assert_eq!(result.into_snapshot().unwrap_err(), expected);
        assert_eq!(
            peer.state
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(name, _)| name == "sessions_list")
                .count(),
            native_calls
        );
        peer.finish().await;
    }
}

#[tokio::test]
async fn any_reads_provider_pages_without_inventing_native_model_or_path_metadata() {
    let peer = ReadPeer::start(ReplyScenario::ProviderRows).await;
    let snapshot = peer.loader()(peer.request()).await.into_snapshot().unwrap();
    let methods = peer
        .state
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    assert!(methods.iter().any(|name| name == "provider_sessions_list"));
    assert_eq!(snapshot.records.len(), 1);
    assert!(
        matches!(&snapshot.records[0].identity,SessionPickerIdentity::HostedProvider(target) if String::from(target.endpoint.endpoint_id.clone())=="claude-local")
    );
    assert_eq!(snapshot.records[0].cwd.as_deref(), Some("/source/provider"));
    assert!(
        snapshot.records[0].model.is_none()
            && snapshot.records[0].reasoning_effort.is_none()
            && snapshot.records[0].normalized_cwd.is_none()
    );
    peer.finish().await;
}

#[tokio::test]
async fn canceling_the_actual_dispatch_read_prevents_a_late_reply_or_final_epoch_probe() {
    let peer = ReadPeer::start(ReplyScenario::HoldPage).await;
    let read = tokio::spawn(peer.loader()(peer.request()));
    tokio::time::timeout(Duration::from_secs(2), peer.state.native_started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    read.abort();
    assert!(read.await.unwrap_err().is_cancelled());
    assert_eq!(
        peer.state
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == "endpoints_list")
            .count(),
        1
    );
    peer.finish().await;
}

fn native_row(endpoint: &Value, index: usize) -> Value {
    json!({"target":{"endpoint":endpoint,"sessionId":format!("source-{index}")},"name":null,"title":"Literal source title","source":"interactive",
        "workingDirectory":"/source/catalog","observation":{"kind":"stored","updatedAt":"2026-10-11T00:00:00Z"},"model":"literal-source-model","gitBranch":null,"idleSeconds":0})
}

#[tokio::test]
async fn complete_limits_and_sparse_opaque_pages_preserve_every_row_and_request_cursor() {
    for (scenario, row_count, page_count) in [
        (ReplyScenario::RowBudget(4096), 4096, 41),
        (ReplyScenario::PageBudget { complete: true }, 0, 65),
        (ReplyScenario::SparsePages, 1, 3),
    ] {
        let peer = ReadPeer::start(scenario).await;
        let snapshot = peer.loader()(peer.request()).await.into_snapshot().unwrap();
        assert_eq!(snapshot.records.len(), row_count);
        {
            let calls = peer.state.calls.lock().unwrap();
            let pages = calls
                .iter()
                .filter(|(method, _)| method == "sessions_list")
                .collect::<Vec<_>>();
            assert_eq!(pages.len(), page_count);
            assert!(pages[0].1.get("cursor").is_none_or(Value::is_null));
            for (index, page) in pages.iter().enumerate().skip(1) {
                assert_eq!(page.1["cursor"], format!("opaque +/=?☀ {index}"));
                assert_eq!(page.1["endpoint"]["serviceId"], SERVICE);
                assert_eq!(page.1["view"], "stored");
            }
        }
        assert_eq!(peer.inventory_calls(), 2);
        peer.finish().await;
    }
}
