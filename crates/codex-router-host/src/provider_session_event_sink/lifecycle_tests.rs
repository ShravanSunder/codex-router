//! Host lifecycle publication is ordered with replay and visible on return.

use crate::{ExternalProviderLaunch, ExternalProviderRuntime};
use acp_client_runtime::ProviderPersistenceTarget;
use collaboration_protocol::{ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess};
use collaboration_service::{
    ProviderOperationStore, ProviderSessionEventHub, ProviderSessionRecord, SessionEventHub,
};
use message_board::{EndpointId, ServiceId, SessionEndpointRef, SessionId, SessionRef};
use session_event_model::{SessionEvent, SessionState};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

const LIFECYCLE_FIXTURE: &str = r#"
import json,sys
mode=sys.argv[1]
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,
 'agentCapabilities':{'loadSession':True,'sessionCapabilities':{'resume':{}}},
 'agentInfo':{'name':'hub-lifecycle-fixture','version':'1'}}})
request=read()
if mode=='create':
 assert request['method']=='session/new',request
 send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
elif mode=='load':
 assert request['method']=='session/load',request
 for kind,text in [('user_message_chunk','Earlier input'),('agent_message_chunk','Earlier output')]:
  send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':kind,'content':{'type':'text','text':text}}}})
 send({'jsonrpc':'2.0','id':request['id'],'result':{}})
elif mode=='resume':
 assert request['method']=='session/resume',request
 send({'jsonrpc':'2.0','id':request['id'],'result':{}})
else: raise AssertionError(mode)
sys.stdin.read()
"#;

type Fixture = (
    tempfile::TempDir,
    Arc<Mutex<ProviderOperationStore>>,
    Arc<ProviderSessionEventHub>,
    ExternalProviderRuntime,
    SessionRef,
);

async fn fixture(mode: &str) -> Fixture {
    let root = tempfile::tempdir().expect("fixture root");
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    let hub = Arc::new(ProviderSessionEventHub::new(Arc::clone(&store)));
    let endpoint = SessionEndpointRef {
        service_id: ServiceId::try_from("00000000-0000-4000-8000-000000000001".to_owned())
            .expect("service"),
        endpoint_id: EndpointId::try_from("cursor-local".to_owned()).expect("endpoint"),
    };
    let session = SessionRef {
        endpoint: endpoint.clone(),
        session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
    };
    let runtime = ExternalProviderRuntime::initialize_with_mcp_http_and_hub(
        ExternalProviderLaunch {
            persistence_target: ProviderPersistenceTarget::Unspecified,
            executable: PathBuf::from("/usr/bin/python3"),
            arguments: vec![
                "-u".into(),
                "-c".into(),
                LIFECYCLE_FIXTURE.into(),
                mode.into(),
            ],
            environment: Vec::new(),
        },
        "fixture",
        "http://127.0.0.1:1/mcp",
        Arc::clone(&hub),
        endpoint,
    )
    .await
    .expect("runtime");
    (root, store, hub, runtime, session)
}

async fn record_session(store: &Arc<Mutex<ProviderOperationStore>>, session: &SessionRef) {
    let target: collaboration_protocol::SessionRef =
        serde_json::from_value(serde_json::to_value(session).expect("session JSON"))
            .expect("control SessionRef");
    let mut owner = target.clone();
    owner.session_id =
        collaboration_protocol::SessionId::try_from("owner-session".to_owned()).expect("owner ID");
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned()).expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: owner.clone().into(),
            approver: owner.into(),
            updated_at_ms: 1,
        })
        .await
        .expect("session record");
}

async fn assert_listed_idle(hub: &ProviderSessionEventHub, session: &SessionRef) {
    let rows = hub
        .sessions(session.endpoint.clone())
        .await
        .expect("hub inventory");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, SessionState::Idle);
    let attached = hub.attach(session.clone()).await.expect("hub attach");
    assert!(matches!(
        attached.snapshot.get(attached.snapshot.len().saturating_sub(2)).map(|event| &event.event),
        Some(SessionEvent::CapabilitiesChanged { capabilities })
            if capabilities.elicitation
                && matches!(capabilities.auth_status, session_event_model::ProviderAuthStatus::NotReported)
    ));
    assert!(matches!(
        attached.snapshot.last().map(|event| &event.event),
        Some(SessionEvent::StateChanged {
            state: SessionState::Idle
        })
    ));
}

#[tokio::test]
async fn create_publishes_idle_before_the_session_is_returned() {
    let (_root, store, hub, runtime, session) = fixture("create").await;
    let provider_session_id = runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
    assert_eq!(provider_session_id, "fixture-session");
    record_session(&store, &session).await;
    assert_listed_idle(&hub, &session).await;
    runtime.shutdown().await;
}

#[tokio::test]
async fn load_replays_historical_items_before_publishing_idle() {
    let (_root, store, hub, runtime, session) = fixture("load").await;
    record_session(&store, &session).await;
    runtime
        .load_session("fixture-session".into(), PathBuf::from("/tmp"))
        .await
        .expect("session/load");
    assert_listed_idle(&hub, &session).await;
    let attached = hub.attach(session).await.expect("replayed attach");
    assert!(matches!(
        attached.snapshot.first().map(|event| &event.event),
        Some(SessionEvent::TurnStarted { .. })
    ));
    assert!(
        attached
            .snapshot
            .iter()
            .any(|event| matches!(event.event, SessionEvent::ItemStarted { .. }))
    );
    assert!(matches!(
        attached.snapshot[attached.snapshot.len() - 3].event,
        SessionEvent::TurnEnded { .. }
    ));
    runtime.shutdown().await;
}

#[tokio::test]
async fn resume_invalidates_history_before_publishing_idle() {
    let (_root, store, hub, runtime, session) = fixture("resume").await;
    record_session(&store, &session).await;
    runtime
        .resume_session("fixture-session".into(), PathBuf::from("/tmp"))
        .await
        .expect("session/resume");
    assert_listed_idle(&hub, &session).await;
    let attached = hub.attach(session).await.expect("resumed attach");
    assert_eq!(attached.snapshot.len(), 2);
    runtime.shutdown().await;
}
