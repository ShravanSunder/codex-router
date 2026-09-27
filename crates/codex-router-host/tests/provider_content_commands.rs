#![allow(clippy::expect_used)]
//! Host content admission preserves actor authority and ACP block order.

use codex_router_host::{
    ExternalProviderBinding, ExternalProviderLaunch, ExternalProviderRuntime,
    ExternalProviderSupervisor, ProviderAcpDeliveryRoute, ProviderPromptContentsError,
    ProviderQueueAdmissionError,
};
use collaboration_protocol::{
    ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess, SessionId,
};
use collaboration_service::{ProviderOperationStore, ProviderSessionRecord};
use session_event_model::{InputId, PromptContent};
use std::{path::PathBuf, sync::Arc};

#[path = "support/provider_acp_message_fifo_support.rs"]
#[allow(dead_code)]
mod support;
use support::{NoLivePeer, available_directory, provider_binding, target};

fn two_block_fixture() -> ExternalProviderLaunch {
    let script = r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'content-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt',request
assert request['params']['prompt']==[
 {'type':'text','text':'Read'},
 {'type':'resource_link','uri':'https://example.test/context','name':'context'}],request['params']['prompt']
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".into(), script.into()],
        environment: Vec::new(),
    }
}

#[tokio::test]
async fn non_creator_content_prompt_is_accepted_after_capability_validation() {
    let root = tempfile::tempdir().expect("provider root");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(two_block_fixture())
        .await
        .expect("fixture provider");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
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
            working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned()).expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: target.clone().into(),
            approver: target.clone().into(),
            updated_at_ms: 1,
        })
        .await
        .expect("session record");
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
    let contents = || {
        vec![
            PromptContent::text("Read".into()).expect("text"),
            PromptContent::resource_link(
                "https://example.test/context".into(),
                "context".into(),
                None,
            )
            .expect("resource link"),
        ]
    };
    let mut other_actor = target.clone();
    other_actor.session_id = SessionId::try_from("other-session".to_owned()).expect("actor ID");
    let unsupported = route
        .prompt_contents(
            target.clone(),
            other_actor.clone().into(),
            InputId::generate(),
            vec![PromptContent::image("image/png".into(), "aGVsbG8=".into(), None).expect("image")],
        )
        .await;
    assert!(matches!(
        unsupported,
        Err(ProviderPromptContentsError::Admission(
            ProviderQueueAdmissionError::UnsupportedContent {
                content_type: "image"
            }
        ))
    ));
    let submitted = route
        .prompt_contents(
            target.clone(),
            other_actor.into(),
            InputId::generate(),
            contents(),
        )
        .await
        .expect("two blocks admitted");
    let settled = collaboration_service::ProviderConversationBackend::wait(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationWaitRequest {
            operation_id: submitted.operation.operation_id,
            timeout_seconds: collaboration_protocol::PositiveSeconds::try_from(2)
                .expect("wait seconds"),
        },
    )
    .await
    .expect("two-block prompt settles");
    assert!(matches!(
        settled.output,
        collaboration_protocol::ConversationOperationWaitOutput::Available { .. }
    ));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

fn cancellable_prompt_fixture(dispatch_log: &std::path::Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,sys
log={:?}
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{}},'agentInfo':{{'name':'cancel-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
with open(log,'a') as marker: marker.write('prompt\n')
cancel=json.loads(sys.stdin.readline())
assert cancel['method']=='session/cancel'
with open(log,'a') as marker: marker.write('cancel\n')
print(json.dumps({{'jsonrpc':'2.0','id':prompt['id'],'result':{{'stopReason':'cancelled'}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
        dispatch_log.display().to_string()
    );
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".into(), script],
        environment: Vec::new(),
    }
}

#[tokio::test]
async fn active_control_turn_cancel_uses_operation_bound_path_and_settles_after_agent_stop() {
    let root = tempfile::tempdir().expect("provider root");
    let dispatch_log = root.path().join("cancel.log");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(cancellable_prompt_fixture(&dispatch_log))
        .await
        .expect("fixture provider");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
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
            working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned()).expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: target.clone().into(),
            approver: target.clone().into(),
            updated_at_ms: 1,
        })
        .await
        .expect("session record");
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
    let idle_cancel = route
        .cancel_active_turn(target.clone(), target.clone().into())
        .await;
    assert!(matches!(
        idle_cancel,
        Err(codex_router_host::ProviderCancelActiveTurnError::NoActiveTurn)
    ));
    let prompt = collaboration_service::ProviderConversationBackend::prompt(
        supervisor.as_ref(),
        collaboration_protocol::ConversationPromptRequest {
            input_id: None,
            operation_id: collaboration_protocol::OperationId::generate(),
            target: target.clone(),
            generation: Some(binding.generation),
            requested_by: target.clone().into(),
            approver: target.clone().into(),
            prompt: collaboration_protocol::MessageContent::HumanUser {
                text: collaboration_protocol::MessageText::try_from("hold".to_owned())
                    .expect("prompt text"),
            },
        },
    )
    .await
    .expect("Control prompt admitted");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if std::fs::read_to_string(&dispatch_log).is_ok_and(|text| text.contains("prompt\n")) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("agent receives Control prompt");
    let mut other_actor = target.clone();
    other_actor.session_id = SessionId::try_from("other-session".to_owned()).expect("actor ID");
    let cancel = route
        .cancel_active_turn(target.clone(), other_actor.into())
        .await
        .expect("active turn cancel admitted");
    let cancelled = collaboration_service::ProviderConversationBackend::wait(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationWaitRequest {
            operation_id: cancel.operation.operation_id,
            timeout_seconds: collaboration_protocol::PositiveSeconds::try_from(2)
                .expect("wait seconds"),
        },
    )
    .await
    .expect("cancel operation settles");
    assert!(matches!(
        cancelled.output,
        collaboration_protocol::ConversationOperationWaitOutput::Available {
            settlement: collaboration_protocol::ConversationOperationSettlement::CancelRequested { .. }
        }
    ));
    let turn = collaboration_service::ProviderConversationBackend::wait(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationWaitRequest {
            operation_id: prompt.operation.operation_id,
            timeout_seconds: collaboration_protocol::PositiveSeconds::try_from(2)
                .expect("wait seconds"),
        },
    )
    .await
    .expect("cancelled turn settles");
    assert!(matches!(
        turn.output,
        collaboration_protocol::ConversationOperationWaitOutput::Available {
            settlement: collaboration_protocol::ConversationOperationSettlement::PromptCompleted {
                stop_reason: collaboration_protocol::ProviderPromptStopReason::Cancelled,
                ..
            }
        }
    ));
    assert_eq!(
        std::fs::read_to_string(dispatch_log).expect("dispatch log"),
        "prompt\ncancel\n"
    );
    assert!(matches!(
        route
            .cancel_active_turn(target.clone(), target.into())
            .await,
        Err(codex_router_host::ProviderCancelActiveTurnError::NoActiveTurn)
    ));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}
