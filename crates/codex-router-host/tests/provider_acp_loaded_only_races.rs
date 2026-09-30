#![allow(clippy::expect_used)]
//! Real-provider races where LoadedOnly must refuse a newly unloaded session.

use agent_automation::{RouteEffectEvidence, SubmissionEffect};
use claude_code_peer_messaging::{ClaudeCodePeerSocket, ClaudeCodeSessionRegistry};
use codex_router_host::{
    ClaudeCodePeerDeliveryRoute, ExternalProviderBinding, ExternalProviderLaunch,
    ExternalProviderRuntime, ExternalProviderSupervisor, LiveSessionOwnership,
    LiveSessionOwnershipCheck, ProviderAcpDeliveryRoute,
};
use collaboration_protocol::{
    CodexGeneration, ConversationCloseRequest, ConversationOperationSettlement,
    ConversationOperationWaitOutput, ConversationOperationWaitRequest, DeliveryOutcome, EndpointId,
    OperationId, PositiveSeconds, ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess,
    SessionRef,
};
use collaboration_service::{
    AttemptEvidenceSink, DeliveryContractError, DeliveryFuture, LoadPolicy, NOT_LOADED_REASON,
    ProviderConversationBackend, ProviderOperationStore, ProviderSessionRecord, RoutePresence,
    SessionDeliveryRoute, SessionDeliveryRouter, TargetPresence, TargetPresenceProbe,
};
use sqlx::Connection as _;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[path = "support/provider_acp_message_fifo_support.rs"]
#[allow(dead_code)]
mod provider_acp_message_fifo_support;
use provider_acp_message_fifo_support::{
    NoLivePeer, RecordedEvidence, available_directory, provider_binding, refusing_load_fixture,
    request, target,
};

struct LivePeer;

impl LiveSessionOwnershipCheck for LivePeer {
    fn check<'a>(&'a self, _target: &'a SessionRef) -> DeliveryFuture<'a, LiveSessionOwnership> {
        Box::pin(async { Ok(LiveSessionOwnership::LiveUnsupported) })
    }
}

fn no_load_fixture() -> ExternalProviderLaunch {
    let script = r#"
import json,sys
request=json.loads(sys.stdin.readline())
assert request['method']=='initialize'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,
 'agentCapabilities':{},'agentInfo':{'name':'no-load-fixture','version':'1'}}}),flush=True)
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script.to_owned()],
        environment: Vec::new(),
    }
}

async fn store_with_session_record(
    root: &Path,
    target: &SessionRef,
) -> Arc<tokio::sync::Mutex<ProviderOperationStore>> {
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(root.display().to_string())
                .expect("working directory"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: target.clone().into(),
            approver: target.clone().into(),
            updated_at_ms: 1,
        })
        .await
        .expect("provider session record");
    store
}

fn close_and_load_fixture(close_marker: &Path, load_marker: &Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,
 'agentCapabilities':{{'loadSession':True,'sessionCapabilities':{{'close':{{}}}}}},
 'agentInfo':{{'name':'close-load-fixture','version':'1'}}}}}})
request=read()
assert request['method']=='session/new'
send({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})
for line in sys.stdin:
 request=json.loads(line)
 if request['method']=='session/close':
  with open({:?},'a') as marker: marker.write('session/close\\n')
 elif request['method']=='session/load':
  with open({:?},'a') as marker: marker.write('session/load\\n')
 else:
  raise AssertionError(request['method'])
 send({{'jsonrpc':'2.0','id':request['id'],'result':{{}}}})
"#,
        close_marker.display().to_string(),
        load_marker.display().to_string(),
    );
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

struct CloseSessionAtDispatch {
    supervisor: Arc<ExternalProviderSupervisor>,
    target: SessionRef,
    evidence: tokio::sync::Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
}

impl AttemptEvidenceSink for CloseSessionAtDispatch {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        let should_close = matches!(
            &evidence,
            RouteEffectEvidence::ProviderAcp(provider)
                if provider.submission == SubmissionEffect::Dispatching
        );
        let supervisor = Arc::clone(&self.supervisor);
        let target = self.target.clone();
        Box::pin(async move {
            if should_close {
                let operation_id = OperationId::generate();
                supervisor
                    .close(ConversationCloseRequest {
                        operation_id: operation_id.clone(),
                        target: target.clone(),
                        generation: None,
                        requested_by: target.clone().into(),
                        approver: target.clone().into(),
                    })
                    .await
                    .map_err(|_| DeliveryContractError::ClientOperation)?;
                let timeout_seconds = PositiveSeconds::try_from(5)
                    .map_err(|_| DeliveryContractError::ClientOperation)?;
                let closed = supervisor
                    .wait(ConversationOperationWaitRequest {
                        operation_id,
                        timeout_seconds,
                    })
                    .await
                    .map_err(|_| DeliveryContractError::ClientOperation)?;
                if !matches!(
                    closed.output,
                    ConversationOperationWaitOutput::Available {
                        settlement: ConversationOperationSettlement::Closed { target: closed_target },
                    } if closed_target == target
                ) {
                    return Err(DeliveryContractError::ClientOperation);
                }
            }
            self.evidence.lock().await.push(evidence);
            Ok(())
        })
    }
}

#[tokio::test]
async fn loaded_only_rechecks_after_reclaim_when_conversation_close_unloads_session() {
    let root = tempfile::tempdir().expect("provider root");
    let close_marker = root.path().join("close-marker.txt");
    let load_marker = root.path().join("load-marker.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime =
        ExternalProviderRuntime::initialize(close_and_load_fixture(&close_marker, &load_marker))
            .await
            .expect("fixture provider");
    assert_eq!(
        runtime
            .create_session(root.path().to_owned())
            .await
            .expect("session/new"),
        "fixture-session"
    );
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("working directory"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: (target.clone()).into(),
            approver: (target.clone()).into(),
            updated_at_ms: 1,
        })
        .await
        .expect("provider session record");
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding.clone(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );
    assert_eq!(
        route.presence(&target).await.expect("loaded presence"),
        RoutePresence::Running
    );
    let sink = CloseSessionAtDispatch {
        supervisor: Arc::clone(&supervisor),
        target: target.clone(),
        evidence: tokio::sync::Mutex::new(Vec::new()),
    };
    let mut request = request(target, "held notification");
    request.load_policy = LoadPolicy::LoadedOnly;

    let receipt = route
        .deliver(request, &sink)
        .await
        .expect("loaded-only delivery receipt");

    assert!(matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ref reason,
        } if reason == NOT_LOADED_REASON
    ));
    assert!(
        close_marker.exists(),
        "ConversationClose must close the fixture session"
    );
    assert!(
        !load_marker.exists(),
        "LoadedOnly must not call session/load after close"
    );
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

#[tokio::test]
async fn loaded_only_reports_non_retryable_unsupported_load_after_can_load_claim() {
    let root = tempfile::tempdir().expect("provider root");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(no_load_fixture())
        .await
        .expect("fixture provider");
    let store = store_with_session_record(root.path(), &target).await;
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding.clone(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );
    let mut request = request(target, "held batch");
    request.load_policy = LoadPolicy::LoadedOnly;

    let receipt = route
        .deliver(
            request,
            &RecordedEvidence(tokio::sync::Mutex::new(Vec::new())),
        )
        .await
        .expect("loaded-only receipt");

    assert!(matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: false,
            ref reason,
        } if reason == "unsupported: load"
    ));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

#[tokio::test]
async fn loaded_only_preserves_live_elsewhere_reason_after_can_load_claim() {
    let root = tempfile::tempdir().expect("provider root");
    let load_marker = root.path().join("load-marker.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(refusing_load_fixture(&load_marker))
        .await
        .expect("fixture provider");
    let store = store_with_session_record(root.path(), &target).await;
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding.clone(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(LivePeer),
    );
    let mut request = request(target, "held batch");
    request.load_policy = LoadPolicy::LoadedOnly;

    let receipt = route
        .deliver(
            request,
            &RecordedEvidence(tokio::sync::Mutex::new(Vec::new())),
        )
        .await
        .expect("loaded-only receipt");

    assert!(matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ref reason,
        } if reason == "session became live elsewhere"
    ));
    assert!(!load_marker.exists());
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

struct RemoveSessionRecordAtDispatch {
    database_path: PathBuf,
    target: SessionRef,
}

impl AttemptEvidenceSink for RemoveSessionRecordAtDispatch {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        let should_remove = matches!(
            &evidence,
            RouteEffectEvidence::ProviderAcp(provider)
                if provider.submission == SubmissionEffect::Dispatching
        );
        let database_path = self.database_path.display().to_string();
        let target = self.target.clone();
        Box::pin(async move {
            if should_remove {
                let mut connection = sqlx::SqliteConnection::connect(&database_path)
                    .await
                    .map_err(|_| DeliveryContractError::ClientOperation)?;
                sqlx::query(
                    "DELETE FROM provider_session_records WHERE target_service_id=? AND target_endpoint_id=? AND target_session_id=?",
                )
                .bind(String::from(target.endpoint.service_id.clone()))
                .bind(String::from(target.endpoint.endpoint_id.clone()))
                .bind(String::from(target.session_id.clone()))
                .execute(&mut connection)
                .await
                .map_err(|_| DeliveryContractError::ClientOperation)?;
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn loaded_only_reports_non_retryable_missing_record_after_can_load_claim() {
    let root = tempfile::tempdir().expect("provider root");
    let load_marker = root.path().join("load-marker.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(refusing_load_fixture(&load_marker))
        .await
        .expect("fixture provider");
    let store = store_with_session_record(root.path(), &target).await;
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding.clone(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        Arc::clone(&store),
        Arc::new(NoLivePeer),
    );
    let mut request = request(target.clone(), "held batch");
    request.load_policy = LoadPolicy::LoadedOnly;
    let sink = RemoveSessionRecordAtDispatch {
        database_path: root.path().join("operations.sqlite"),
        target: target.clone(),
    };

    let receipt = route
        .deliver(request, &sink)
        .await
        .expect("loaded-only receipt");

    assert!(matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: false,
            ref reason,
        } if reason == "provider session record is missing"
    ));
    assert!(
        store
            .lock()
            .await
            .session_record(&target)
            .await
            .expect("record read")
            .is_none()
    );
    assert!(!load_marker.exists());
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

#[tokio::test]
async fn provider_and_absent_peer_presence_aggregates_to_unreachable() {
    let root = tempfile::tempdir().expect("fixture root");
    let mut target = target();
    target.endpoint.endpoint_id =
        EndpointId::try_from("claude-local".to_owned()).expect("Claude endpoint");
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(no_load_fixture())
        .await
        .expect("fixture provider");
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding.clone(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    let provider_route = Arc::new(ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    ));
    let peer_route = Arc::new(ClaudeCodePeerDeliveryRoute::new(
        target.endpoint.clone(),
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    ));
    let routes: Vec<Arc<dyn SessionDeliveryRoute>> = vec![provider_route.clone(), peer_route];
    let router = SessionDeliveryRouter::new(routes);

    assert_eq!(
        router.presence(&target).await.expect("router presence"),
        TargetPresence::Unreachable {
            reason: "no route holds or can load this session".to_owned(),
        }
    );
    provider_route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}
