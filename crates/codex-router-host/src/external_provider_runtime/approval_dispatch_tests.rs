#![allow(clippy::panic_in_result_fn)]

use super::*;
use collaboration_protocol::{
    ApprovalDecideParams, ApprovalDecision, CodexGeneration, DeliveryOutcome, DeliveryReceipt,
    EndpointId, EndpointRef, GenerationNumber, OperationId, SessionId, SessionReachability,
    SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
    DeliveryRequest, NativeControlBackend, NativeGenerationGate, ServiceApprovalBroker,
    SessionMessageDelivery,
};
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct AcceptedApprovalNoticeDelivery;

impl SessionMessageDelivery for AcceptedApprovalNoticeDelivery {
    fn deliver<'a>(
        &'a self,
        _: DeliveryRequest,
        _: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async {
            Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::Started,
                reachability: Some(SessionReachability::ProviderAcp),
                client: None,
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async { Ok(AttemptReconciliation::StillUnknown) })
    }
}

fn permission_dispatch_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
def send(value):
    print(json.dumps(value)); sys.stdout.flush()
initialize=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':initialize['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'two-session-permission-fixture','version':'1'}}})
first_create=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':first_create['id'],'result':{'sessionId':'session-a'}})
second_create=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':second_create['id'],'result':{'sessionId':'session-b'}})
first_prompt=json.loads(sys.stdin.readline())
assert first_prompt['method']=='session/prompt'
assert first_prompt['params']['sessionId']=='session-a'
send({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':'session-a','toolCall':{'toolCallId':'permission-a','title':'Run an approved command','kind':'execute'},'options':[{'optionId':'allow-a','name':'Allow once','kind':'allow_once'}]}})
second_prompt=json.loads(sys.stdin.readline())
assert second_prompt['method']=='session/prompt'
assert second_prompt['params']['sessionId']=='session-b'
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'session-b','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'session-b-replied'}}}})
send({'jsonrpc':'2.0','id':second_prompt['id'],'result':{'stopReason':'end_turn'}})
permission=json.loads(sys.stdin.readline())
assert permission['id']==91
assert permission['result']['outcome']['outcome']=='selected'
assert permission['result']['outcome']['optionId']=='allow-a'
send({'jsonrpc':'2.0','method':'$/cancel_request','params':{'requestId':91}})
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'session-a','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'session-a-approved'}}}})
send({'jsonrpc':'2.0','id':first_prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: Vec::new(),
    }
}

fn permission_cancellation_fixture(cancellation_source: &str) -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
cancel_source=sys.argv[1]
def send(value):
    print(json.dumps(value)); sys.stdout.flush()
initialize=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':initialize['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'permission-cancellation-fixture','version':'1'}}})
create=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':create['id'],'result':{'sessionId':'session-a'}})
prompt=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':'session-a','toolCall':{'toolCallId':'permission-a','title':'Run an approved command','kind':'execute'},'options':[{'optionId':'allow-a','name':'Allow once','kind':'allow_once'}]}})
if cancel_source=='peer':
    send({'jsonrpc':'2.0','method':'$/cancel_request','params':{'requestId':91}})
permission=json.loads(sys.stdin.readline())
assert permission['id']==91
assert permission['result']['outcome']['outcome']=='cancelled'
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'session-a','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'permission-cancelled'}}}})
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'cancelled'}})
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec![
            "-c".to_owned(),
            fixture.to_owned(),
            cancellation_source.to_owned(),
        ],
        environment: Vec::new(),
    }
}

fn permission_during_steer_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
def send(value):
    print(json.dumps(value)); sys.stdout.flush()
initialize=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':initialize['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'_meta':{'steering':{'supported':True}},'agentInfo':{'name':'permission-steer-fixture','version':'1'}}})
create=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':create['id'],'result':{'sessionId':'session-a'}})
prompt=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':'session-a','toolCall':{'toolCallId':'permission-a','title':'Run an approved command','kind':'execute'},'options':[{'optionId':'allow-a','name':'Allow once','kind':'allow_once'}]}})
steer=json.loads(sys.stdin.readline())
assert steer['method']=='_session/steering'
send({'jsonrpc':'2.0','id':steer['id'],'result':{'outcome':'injected'}})
permission=json.loads(sys.stdin.readline())
assert permission['id']==91
assert permission['result']['outcome']['outcome']=='selected'
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'session-a','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'session-a-steered-and-approved'}}}})
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: Vec::new(),
    }
}

fn permission_during_cancel_fixture() -> ExternalProviderLaunch {
    // ACP v1 prompt-turn.mdx:336-350 requires the client to answer every
    // pending permission with `cancelled` after it sends session/cancel.
    acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion": 1}))
        .respond("initialize", serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {},
            "agentInfo": {"name": "permission-cancel-fixture", "version": "1"}
        }))
        .expect_request("create", "session/new", serde_json::json!({}))
        .respond("create", serde_json::json!({"sessionId": "session-a"}))
        .expect_request("prompt", "session/prompt", serde_json::json!({"sessionId": "session-a"}))
        .send(serde_json::json!({
            "jsonrpc": "2.0", "id": 91, "method": "session/request_permission",
            "params": {"sessionId": "session-a", "toolCall": {"toolCallId": "permission-a", "title": "Run an approved command", "kind": "execute"},
                "options": [{"optionId": "allow-a", "name": "Allow once", "kind": "allow_once"}]}
        }))
        .expect_message(serde_json::json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "session-a"}}))
        .expect_message(serde_json::json!({"jsonrpc": "2.0", "id": 91, "result": {"outcome": {"outcome": "cancelled"}}}))
        .send(serde_json::json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "session-a", "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "session-a-cancelled"}}}}))
        .respond("prompt", serde_json::json!({"stopReason": "cancelled"}))
        .launch()
}

async fn wait_for_pending_approval<TPromptFuture>(
    broker: &ServiceApprovalBroker,
    mut pending_request: Pin<&mut TPromptFuture>,
) -> TestResult
where
    TPromptFuture:
        Future<Output = Result<ExternalProviderPromptOutcome, ExternalProviderRuntimeError>>,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = pending_request.as_mut() => {
                    return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                        format!("permission prompt settled before its cancellation test: {result:?}").into()
                    );
                }
                () = tokio::task::yield_now() => {}
            }
            if !broker.list(true).await.approvals.is_empty() {
                return Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
            }
        }
    })
    .await
    .map_err(|_| "permission request did not become pending")??;
    Ok(())
}

fn session_ref(
    service_id: &UuidIdentity,
    endpoint_id: &str,
    session_id: &str,
) -> TestResult<SessionRef> {
    Ok(SessionRef {
        endpoint: EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: EndpointId::try_from(endpoint_id.to_owned())?,
        },
        session_id: SessionId::try_from(session_id.to_owned())?,
    })
}

async fn approval_broker_fixture(
    root: &tempfile::TempDir,
    service_id: &UuidIdentity,
    generation: &CodexGeneration,
) -> TestResult<Arc<ServiceApprovalBroker>> {
    let gate = NativeGenerationGate::default();
    gate.activate(
        generation.clone(),
        root.path().join("unused-native.sock"),
        None,
    )?;
    let broker = ServiceApprovalBroker::load(
        service_id.clone(),
        NativeControlBackend {
            endpoint: EndpointRef {
                service_id: service_id.clone(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
            },
            gate,
            codex_home: root.path().to_owned(),
        },
        root.path().join("approval-routes.json"),
    )
    .await?;
    broker.install_session_delivery(Arc::new(AcceptedApprovalNoticeDelivery))?;
    Ok(broker)
}

#[tokio::test]
async fn permission_decision_survives_late_agent_withdrawal_and_peer_turn_progresses() -> TestResult
{
    let root = tempfile::tempdir()?;
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    let runtime = ExternalProviderRuntime::initialize(permission_dispatch_fixture()).await?;
    let broker = approval_broker_fixture(&root, &service_id, &generation).await?;
    runtime.install_approval_broker(Arc::clone(&broker)).await;
    runtime.create_session(root.path().to_owned()).await?;
    runtime.create_session(root.path().to_owned()).await?;

    let requester_and_approver = session_ref(&service_id, "cursor-local", "session-b")?;
    let target_a = session_ref(&service_id, "cursor-local", "session-a")?;
    let prompt_a = runtime.prompt_with_approval_context(
        "session-a".to_owned(),
        "Run a command requiring permission.".to_owned(),
        ExternalProviderApprovalContext {
            requester: requester_and_approver.clone(),
            approver: requester_and_approver.clone(),
            target: target_a,
            operation_id: OperationId::generate(),
            binding_generation: generation.clone(),
            binding_retirement: CancellationToken::new(),
        },
    );
    tokio::pin!(prompt_a);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut prompt_a => {
                    return Err(format!("permission prompt settled before approval: {result:?}").into());
                }
                () = tokio::task::yield_now() => {}
            }
            if !broker.list(true).await.approvals.is_empty() {
                break Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
            }
        }
    })
    .await
    .map_err(|_| "permission request did not become pending")??;

    let prompt_b = runtime.prompt_with_approval_context(
        "session-b".to_owned(),
        "Reply while session A is waiting for permission.".to_owned(),
        ExternalProviderApprovalContext {
            requester: requester_and_approver.clone(),
            approver: requester_and_approver.clone(),
            target: requester_and_approver.clone(),
            operation_id: OperationId::generate(),
            binding_generation: generation,
            binding_retirement: CancellationToken::new(),
        },
    );
    let prompt_b_outcome = tokio::time::timeout(Duration::from_secs(2), prompt_b).await??;
    assert_eq!(prompt_b_outcome.output, "session-b-replied");

    let pending = broker
        .list(true)
        .await
        .approvals
        .pop()
        .expect("pending approval");
    broker
        .decide(ApprovalDecideParams {
            request_id: pending.request_id,
            decision: ApprovalDecision::Allow,
            actor: requester_and_approver,
        })
        .await?;
    let prompt_a_outcome = tokio::time::timeout(Duration::from_secs(2), &mut prompt_a).await??;
    assert_eq!(prompt_a_outcome.output, "session-a-approved");
    assert_eq!(
        broker.list(false).await.approvals[0].state,
        collaboration_protocol::ApprovalState::Decided
    );

    runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn expired_approval_broker_refusal_carries_typed_operation_reason() -> TestResult {
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    let runtime =
        ExternalProviderRuntime::initialize(permission_cancellation_fixture("expired")).await?;
    runtime.set_endpoint_id("cursor-local".to_owned()).await;
    runtime.create_session(PathBuf::from("/tmp")).await?;
    let root = tempfile::tempdir()?;
    let broker = approval_broker_fixture(&root, &service_id, &generation).await?;
    runtime.install_approval_broker(broker.clone()).await;
    drop(broker);
    let operation_id = OperationId::generate();
    let outcome = runtime
        .prompt_with_approval_context(
            "session-a".to_owned(),
            "Run a command requiring permission.".to_owned(),
            ExternalProviderApprovalContext {
                requester: session_ref(&service_id, "codex-local", "requester")?,
                approver: session_ref(&service_id, "codex-local", "approver")?,
                target: session_ref(&service_id, "cursor-local", "session-a")?,
                operation_id,
                binding_generation: generation,
                binding_retirement: CancellationToken::new(),
            },
        )
        .await?;

    assert_eq!(outcome.stop_reason, ProviderPromptStopReason::Cancelled);
    assert_eq!(
        outcome.permission_refusal_reason,
        Some(ExternalProviderApprovalRefusalReason::ApprovalBrokerUnavailable)
    );
    assert_eq!(
        runtime.approval_refusal_warnings(),
        vec![ExternalProviderApprovalRefusalWarning {
            endpoint: "cursor-local".to_owned(),
            provider_session_id: "session-a".to_owned(),
            method: "session/request_permission",
            reason_code: ExternalProviderApprovalRefusalReason::ApprovalBrokerUnavailable,
        }]
    );
    runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn peer_cancellation_and_binding_retirement_settle_permission_history_once() -> TestResult {
    for cancellation_source in ["peer", "retirement"] {
        let root = tempfile::tempdir()?;
        let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
        let generation = CodexGeneration {
            service_epoch: service_id.clone(),
            generation: GenerationNumber::try_from(1)?,
        };
        let runtime = ExternalProviderRuntime::initialize(permission_cancellation_fixture(
            cancellation_source,
        ))
        .await?;
        let broker = approval_broker_fixture(&root, &service_id, &generation).await?;
        runtime.install_approval_broker(Arc::clone(&broker)).await;
        runtime.create_session(root.path().to_owned()).await?;
        let requester = session_ref(&service_id, "codex-local", "approver")?;
        let target = session_ref(&service_id, "cursor-local", "session-a")?;
        let retirement = CancellationToken::new();
        let mut prompt = Box::pin(runtime.prompt_with_approval_context(
            "session-a".to_owned(),
            "Run a command requiring permission.".to_owned(),
            ExternalProviderApprovalContext {
                requester: requester.clone(),
                approver: requester,
                target,
                operation_id: OperationId::generate(),
                binding_generation: generation,
                binding_retirement: retirement.clone(),
            },
        ));
        if cancellation_source == "retirement" {
            wait_for_pending_approval(&broker, prompt.as_mut()).await?;
            retirement.cancel();
        }
        let prompt_result = tokio::time::timeout(Duration::from_secs(5), &mut prompt).await?;
        if cancellation_source == "retirement" {
            assert!(matches!(
                prompt_result,
                Err(ExternalProviderRuntimeError::TransportFailure)
            ));
        } else {
            let outcome = prompt_result?;
            assert_eq!(outcome.stop_reason, ProviderPromptStopReason::Cancelled);
            assert_eq!(outcome.output, "permission-cancelled");
        }
        let history = broker.list(false).await.approvals;
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].state,
            collaboration_protocol::ApprovalState::Cancelled
        );
        assert!(history[0].reason.is_some());
        assert!(broker.list(true).await.approvals.is_empty());
        runtime.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn permission_wait_does_not_block_steering_reply_dispatch() -> TestResult {
    let root = tempfile::tempdir()?;
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    let runtime = ExternalProviderRuntime::initialize(permission_during_steer_fixture()).await?;
    let broker = approval_broker_fixture(&root, &service_id, &generation).await?;
    runtime.install_approval_broker(Arc::clone(&broker)).await;
    runtime.create_session(root.path().to_owned()).await?;
    let requester = session_ref(&service_id, "codex-local", "approver")?;
    let target = session_ref(&service_id, "cursor-local", "session-a")?;
    let mut prompt = Box::pin(runtime.prompt_with_approval_context(
        "session-a".to_owned(),
        "Run a command requiring permission.".to_owned(),
        ExternalProviderApprovalContext {
            requester: requester.clone(),
            approver: requester.clone(),
            target,
            operation_id: OperationId::generate(),
            binding_generation: generation,
            binding_retirement: CancellationToken::new(),
        },
    ));
    wait_for_pending_approval(&broker, prompt.as_mut()).await?;

    let steer = tokio::time::timeout(
        Duration::from_secs(2),
        runtime.steer_session(
            "session-a".to_owned(),
            "Continue with the current approved action.".to_owned(),
        ),
    )
    .await??;
    assert!(matches!(steer, ProviderSteeringOutcome::Injected { .. }));
    let pending = broker
        .list(true)
        .await
        .approvals
        .pop()
        .expect("approval is still pending");
    broker
        .decide(ApprovalDecideParams {
            request_id: pending.request_id,
            decision: ApprovalDecision::Allow,
            actor: requester,
        })
        .await?;
    let outcome = tokio::time::timeout(Duration::from_secs(2), &mut prompt).await??;
    assert_eq!(outcome.output, "session-a-steered-and-approved");
    assert_eq!(
        broker.list(false).await.approvals[0].state,
        collaboration_protocol::ApprovalState::Decided
    );
    runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn permission_wait_observes_cancellation_during_provider_cancel() -> TestResult {
    let root = tempfile::tempdir()?;
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    let runtime = ExternalProviderRuntime::initialize(permission_during_cancel_fixture()).await?;
    let broker = approval_broker_fixture(&root, &service_id, &generation).await?;
    runtime.install_approval_broker(Arc::clone(&broker)).await;
    runtime.create_session(root.path().to_owned()).await?;
    let requester = session_ref(&service_id, "codex-local", "approver")?;
    let target = session_ref(&service_id, "cursor-local", "session-a")?;
    let mut prompt = Box::pin(runtime.prompt_with_approval_context(
        "session-a".to_owned(),
        "Run a command requiring permission.".to_owned(),
        ExternalProviderApprovalContext {
            requester: requester.clone(),
            approver: requester,
            target,
            operation_id: OperationId::generate(),
            binding_generation: generation,
            binding_retirement: CancellationToken::new(),
        },
    ));
    wait_for_pending_approval(&broker, prompt.as_mut()).await?;
    runtime.cancel_active_prompt("session-a".to_owned()).await?;

    let outcome = tokio::time::timeout(Duration::from_secs(5), &mut prompt).await??;
    assert_eq!(outcome.stop_reason, ProviderPromptStopReason::Cancelled);
    assert_eq!(outcome.output, "session-a-cancelled");
    let history = broker.list(false).await.approvals;
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].state,
        collaboration_protocol::ApprovalState::Cancelled
    );
    assert!(history[0].reason.is_some());
    assert!(broker.list(true).await.approvals.is_empty());
    runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn provider_exit_cancels_pending_permission_as_provider_retired() -> TestResult {
    // ACP v1 prompt-turn.mdx:365-367 ends a Turn only at the agent's prompt
    // result. Specification R5 projects connection loss as providerRetired.
    let root = tempfile::tempdir_in("/private/tmp")?;
    let process_id_path = root.path().join("fixture-process-id");
    let fixture = acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion": 1}))
        .respond("initialize", serde_json::json!({"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "loss-fixture", "version": "1"}}))
        .expect_request("create", "session/new", serde_json::json!({}))
        .respond("create", serde_json::json!({"sessionId": "session-a"}))
        .expect_request("prompt", "session/prompt", serde_json::json!({"sessionId": "session-a"}))
        .send(serde_json::json!({"jsonrpc": "2.0", "id": 91, "method": "session/request_permission", "params": {"sessionId": "session-a", "toolCall": {"toolCallId": "permission-a", "title": "Run an approved command", "kind": "execute"}, "options": [{"optionId": "allow-a", "name": "Allow once", "kind": "allow_once"}]}}))
        .wait_for_signal(&process_id_path)
        .exit()
        .record_diagnostics(root.path().join("fixture-diagnostics.txt"))
        .launch();
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    let runtime = ExternalProviderRuntime::initialize(fixture).await?;
    let broker = approval_broker_fixture(&root, &service_id, &generation).await?;
    runtime.install_approval_broker(Arc::clone(&broker)).await;
    runtime.create_session(root.path().to_owned()).await?;
    let approver = session_ref(&service_id, "codex-local", "approver")?;
    let mut prompt = Box::pin(runtime.prompt_with_approval_context(
        "session-a".to_owned(),
        "Run a command requiring permission.".to_owned(),
        ExternalProviderApprovalContext {
            requester: approver.clone(),
            approver,
            target: session_ref(&service_id, "cursor-local", "session-a")?,
            operation_id: OperationId::generate(),
            binding_generation: generation,
            binding_retirement: runtime.retirement(),
        },
    ));
    if let Err(error) = wait_for_pending_approval(&broker, prompt.as_mut()).await {
        let diagnostics = std::fs::read_to_string(root.path().join("fixture-diagnostics.txt"))
            .unwrap_or_default();
        return Err(format!("{error}; fixture: {diagnostics}").into());
    }
    let process_id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(value) = std::fs::read_to_string(&process_id_path)
                && let Ok(process_id) = value.parse::<i32>()
            {
                break process_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| {
        format!(
            "fixture process did not become ready for signal: {}",
            std::fs::read_to_string(root.path().join("fixture-diagnostics.txt"))
                .unwrap_or_default()
        )
    })?;
    let process_id =
        rustix::process::Pid::from_raw(process_id).ok_or("invalid fixture process ID")?;
    rustix::process::kill_process(process_id, rustix::process::Signal::USR1)?;
    let prompt_result = tokio::time::timeout(Duration::from_secs(5), &mut prompt)
        .await
        .map_err(|_| "prompt did not settle after fixture agent exited")?;
    assert!(
        matches!(
            prompt_result,
            Err(ExternalProviderRuntimeError::TransportFailure)
        ),
        "{prompt_result:?}"
    );
    let history_result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let history = broker.list(false).await.approvals;
            if history[0].state == collaboration_protocol::ApprovalState::Cancelled {
                break history;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let history = match history_result {
        Ok(history) => history,
        Err(_) => {
            return Err(format!(
                "approval history did not settle after fixture agent exited: {:?}",
                broker.list(false).await.approvals
            )
            .into());
        }
    };
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].reason.as_deref(), Some("providerRetired"));
    assert!(broker.list(true).await.approvals.is_empty());
    runtime.shutdown().await;
    Ok(())
}
