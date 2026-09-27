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
