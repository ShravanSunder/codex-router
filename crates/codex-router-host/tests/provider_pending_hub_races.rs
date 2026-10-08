//! Real Host, ACP process, approval broker, and Session hub race proof.

use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    ExternalProviderStartup,
};
use collaboration_client::CollaborationClient;
use collaboration_protocol::{
    BoundedObservationRequest, ChannelDescription, CodexGeneration, ConversationCancelRequest,
    ConversationCreateRequest, ConversationOperationSettlement, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, ConversationPromptRequest, EndpointId, EndpointRef,
    MessageContent, MessageText, NativeSessionScope, NativeSessionSource, NativeSessionView,
    OperationId, PositiveSeconds, ProviderIdentity, ProviderRequestedPolicy,
    ProviderSessionListParams, ProviderSessionState, ProviderWorkingDirectory, RouterAccess,
    SessionId, SessionRef,
};
use serde_json::{Value, json};
use std::{collections::VecDeque, os::unix::fs::PermissionsExt as _, path::Path, time::Duration};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[path = "support/provider_conversation_submission.rs"]
#[allow(dead_code)]
mod provider_conversation_submission;
use provider_conversation_submission::{
    spawn_provider_prompt, submit_provider_cancel, submit_provider_create,
};

const PROVIDER: &str = r#"#!/usr/bin/python3
import json,os,socket,sys,threading
control=socket.socket(socket.AF_UNIX)
control.bind(sys.argv[1]); control.listen(1)
release_cancelled_prompt=threading.Event()
def control_requests():
    while True:
        peer,_=control.accept()
        command=peer.recv(1)
        peer.close()
        if command==b'x': os._exit(0)
        if command==b'c': release_cancelled_prompt.set()
threading.Thread(target=control_requests,daemon=True).start()
def send(value): print(json.dumps(value),flush=True)
request=json.loads(sys.stdin.readline())
assert request['method']=='initialize',request
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'pending-hub-fixture','version':'1'}}})
next_session=0
pending={}
responded=set()
for line in sys.stdin:
    request=json.loads(line)
    method=request.get('method')
    if method=='session/new':
        next_session+=1
        send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-'+str(next_session)}})
    elif method=='session/prompt':
        session=request['params']['sessionId']
        if session=='fixture-1':
            pending[session]=request['id']
            send({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':session,'toolCall':{'toolCallId':'permission-tool'},'options':[{'optionId':'allow-once','name':'Allow once','kind':'allow_once'}]}})
            release_cancelled_prompt.wait()
            send({'jsonrpc':'2.0','id':pending.pop(session),'result':{'stopReason':'cancelled'}})
            responded.add(session)
        else:
            send({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})
    elif method=='session/cancel':
        session=request['params']['sessionId']
        if session not in responded:
            send({'jsonrpc':'2.0','id':pending.pop(session),'result':{'stopReason':'cancelled'}})
            responded.add(session)
    elif request.get('id')==91:
        assert request['result']['outcome']['outcome']=='cancelled',request
        if 'fixture-1' not in responded:
            send({'jsonrpc':'2.0','id':pending.pop('fixture-1'),'result':{'stopReason':'cancelled'}})
            responded.add('fixture-1')
    else:
        raise AssertionError(request)
"#;

async fn start_host(root: &Path) -> TestResult<CollaborationRuntime> {
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
    let provider = root.join("pending-hub-provider.py");
    std::fs::write(&provider, PROVIDER)?;
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700))?;
    Ok(CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            owner_human_id: None,
            directory: root.to_owned(),
            codex_home: root.to_owned(),
            backend_socket: root.join("backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: None,
            remote_control_server_name: None,
        },
        vec![ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::claude(
                provider,
                vec![root.join("exit.sock").display().to_string()],
            )?,
        )],
    )
    .await?)
}

async fn create_session(
    client: &CollaborationClient,
    root: &Path,
) -> TestResult<(SessionRef, SessionRef, ProviderIdentity, CodexGeneration)> {
    let inventory = client.list_endpoints().await?;
    let provider = inventory
        .endpoints
        .iter()
        .find(|entry| String::from(entry.endpoint.endpoint_id.clone()) == "claude-local")
        .ok_or("provider endpoint missing")?;
    let generation = provider
        .channels
        .iter()
        .find_map(|channel| match channel {
            ChannelDescription::ExternalProvider {
                binding_generation, ..
            } => Some(CodexGeneration {
                service_epoch: inventory.service_epoch.clone(),
                generation: *binding_generation,
            }),
            _ => None,
        })
        .ok_or("provider generation missing")?;
    let actor = SessionRef {
        endpoint: EndpointRef {
            service_id: provider.endpoint.service_id.clone(),
            endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
        },
        session_id: SessionId::try_from("race-caller".to_owned())?,
    };
    let approver: ProviderIdentity = serde_json::from_value(json!({"humanId":"race-human"}))?;
    let operation_id = OperationId::generate();
    submit_provider_create(
        client,
        ConversationCreateRequest {
            settings: None,
            operation_id: operation_id.clone(),
            endpoint: provider.endpoint.clone(),
            generation: Some(generation.clone()),
            working_directory: ProviderWorkingDirectory::try_from(root.display().to_string())?,
            created_by: actor.clone().into(),
            approver: approver.clone(),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
        },
    )
    .await?;
    let result = client
        .wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
            operation_id,
            timeout_seconds: PositiveSeconds::try_from(3)?,
        })
        .await?;
    let ConversationOperationWaitOutput::Available {
        settlement: ConversationOperationSettlement::Created { target, .. },
    } = result.output
    else {
        return Err(format!("create did not settle: {:?}", result.output).into());
    };
    Ok((target, actor, approver, generation))
}

type PromptTask = tokio::task::JoinHandle<
    Result<
        collaboration_client::ConversationOperationResult,
        collaboration_client::ConversationClientError,
    >,
>;

/// Prompts in the background: the prompt stays pending on its approval while the test acts.
fn prompt(
    client: &CollaborationClient,
    target: &SessionRef,
    actor: &SessionRef,
    approver: &ProviderIdentity,
    generation: &CodexGeneration,
) -> TestResult<(OperationId, PromptTask)> {
    let operation_id = OperationId::generate();
    let task = spawn_provider_prompt(
        client,
        ConversationPromptRequest {
            input_id: None,
            operation_id: operation_id.clone(),
            target: target.clone(),
            generation: Some(generation.clone()),
            requested_by: actor.clone().into(),
            approver: approver.clone(),
            prompt: MessageContent::HumanUser {
                text: MessageText::try_from("request permission".to_owned())?,
            },
        },
    );
    Ok((operation_id, task))
}

/// Follows one provider Session's hub events through short bounded `events_observe` calls,
/// each resuming after the last event within the hub's epoch.
struct HubEventFollow {
    client: CollaborationClient,
    target: SessionRef,
    epoch: Option<u64>,
    after_sequence: Option<u64>,
    pending: VecDeque<Value>,
}

impl HubEventFollow {
    fn new(client: &CollaborationClient, target: &SessionRef) -> Self {
        Self {
            client: client.clone(),
            target: target.clone(),
            epoch: None,
            after_sequence: None,
            pending: VecDeque::new(),
        }
    }

    async fn next_message(&mut self) -> TestResult<Value> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(event);
            }
            let observed = self
                .client
                .observe_provider_session(BoundedObservationRequest {
                    target: self.target.clone(),
                    timeout_seconds: 1,
                    max_events: 64,
                    max_bytes: 1_048_576,
                    after_sequence: self.after_sequence,
                    epoch: self.epoch,
                })
                .await?;
            self.epoch = observed.epoch;
            for event in &observed.events {
                if let Some(sequence) = event.get("sequence").and_then(Value::as_u64) {
                    self.after_sequence = Some(
                        self.after_sequence
                            .map_or(sequence, |after| after.max(sequence)),
                    );
                }
            }
            self.pending.extend(observed.events);
        }
    }
}

async fn wait_for_pending(
    client: &CollaborationClient,
    observer: &mut HubEventFollow,
) -> TestResult<(String, Vec<Value>)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    loop {
        let notification = tokio::time::timeout_at(deadline, observer.next_message())
            .await
            .map_err(|_| format!("no pending approval event; observed: {events:?}"))??;
        let event = notification
            .get("event")
            .cloned()
            .ok_or("missing hub event")?;
        let pending = event.get("kind").and_then(Value::as_str) == Some("interactionRequested");
        events.push(event);
        if pending {
            let pending = client.list_approvals_with_options(true).await?;
            let approval = pending
                .approvals
                .into_iter()
                .next()
                .ok_or("real hub announced pending approval but broker list omitted it")?;
            break Ok((approval.request_id, events));
        }
    }
}

async fn watch_turn(
    observer: &mut HubEventFollow,
    terminal: &'static str,
    mut events: Vec<Value>,
) -> TestResult<Vec<Value>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let notification = tokio::time::timeout_at(deadline, observer.next_message())
            .await
            .map_err(|_| format!("hub did not publish {terminal}; observed: {events:?}"))??;
        let event = notification
            .get("event")
            .cloned()
            .ok_or("missing hub event")?;
        let done = event.get("kind").and_then(Value::as_str) == Some(terminal);
        events.push(event);
        if done {
            break Ok(events);
        }
    }
}

fn assert_interaction_settles_before_turn_end(events: &[Value]) -> TestResult {
    let requested = events
        .iter()
        .position(|event| event.get("kind").and_then(Value::as_str) == Some("interactionRequested"))
        .ok_or("real hub omitted interaction request")?;
    let resolved = events
        .iter()
        .position(|event| event.get("kind").and_then(Value::as_str) == Some("interactionResolved"))
        .ok_or("real hub omitted interaction resolution")?;
    let ended = events
        .iter()
        .position(|event| event.get("kind").and_then(Value::as_str) == Some("turnEnded"))
        .ok_or("real hub omitted turn end")?;
    if requested >= resolved || resolved >= ended {
        return Err(format!("hub event order: {events:?}").into());
    }
    if events
        .iter()
        .any(|event| event.get("kind").and_then(Value::as_str) == Some("resyncRequired"))
    {
        return Err(format!("hub rejected event projection: {events:?}").into());
    }
    Ok(())
}

async fn listed_state(
    client: &CollaborationClient,
    target: &SessionRef,
) -> TestResult<ProviderSessionState> {
    let listing = client
        .list_provider_sessions(ProviderSessionListParams {
            endpoint: target.endpoint.clone(),
            view: NativeSessionView::Stored,
            scope: NativeSessionScope::Any,
            source: NativeSessionSource::All,
            query: None,
            page_size: 10,
            cursor: None,
        })
        .await?;
    let summary = listing
        .sessions
        .into_iter()
        .find(|session| session.target() == target)
        .ok_or("target omitted from provider session list")?;
    match summary {
        collaboration_protocol::ProviderSessionSummary::HostedProvider { state, .. } => Ok(state),
        collaboration_protocol::ProviderSessionSummary::ClaudeCodeInteractive { .. } => {
            Err("expected a Router-hosted provider session".into())
        }
    }
}

#[tokio::test]
async fn provider_exit_resolves_pending_approval_before_lost_turn() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = start_host(root.path()).await?;
    let client = CollaborationClient::connect(root.path(), "exit-control", "1").await?;
    let (target, actor, _, generation) = create_session(&client, root.path()).await?;
    // The approver is another provider Session: the API's prompt names Session approvers,
    // and this one's approval notice waits behind the blocked prompt, so the approval stays
    // pending.
    let (approver_session, _, _, _) = create_session(&client, root.path()).await?;
    let approver = ProviderIdentity::from(approver_session);
    let mut observer = HubEventFollow::new(&client, &target);
    let _prompt = prompt(&client, &target, &actor, &approver, &generation)?;
    let (request_id, initial_events) = wait_for_pending(&client, &mut observer).await?;
    let mut exit = tokio::net::UnixStream::connect(root.path().join("exit.sock")).await?;
    use tokio::io::AsyncWriteExt as _;
    exit.write_all(b"x").await?;
    let events = watch_turn(&mut observer, "turnEnded", initial_events).await?;
    assert_interaction_settles_before_turn_end(&events)?;
    let settled = client.list_approvals_with_options(false).await?;
    if !settled.approvals.iter().any(|approval| {
        approval.request_id == request_id && approval.reason.as_deref() == Some("providerRetired")
    }) {
        return Err(format!(
            "retired approval not visible in detailed list: {:?}",
            settled.approvals
        )
        .into());
    }
    let state = listed_state(&client, &target).await?;
    if state != ProviderSessionState::Unloaded {
        return Err(format!("retired Session remained in {state:?}").into());
    }
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn explicit_cancel_settles_approval_and_keeps_peer_session_usable() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = start_host(root.path()).await?;
    let client = CollaborationClient::connect(root.path(), "cancel-control", "1").await?;
    let (target, actor, _, generation) = create_session(&client, root.path()).await?;
    let (peer, _, _, _) = create_session(&client, root.path()).await?;
    // The peer provider Session approves: the API's prompt names Session approvers, and its
    // approval notice waits behind the blocked prompt, so the approval stays pending.
    let approver = ProviderIdentity::from(peer.clone());
    let mut observer = HubEventFollow::new(&client, &target);
    let (prompt_id, _prompt) = prompt(&client, &target, &actor, &approver, &generation)?;
    let (request_id, initial_events) = wait_for_pending(&client, &mut observer).await?;
    let cancel_id = OperationId::generate();
    submit_provider_cancel(
        &client,
        ConversationCancelRequest {
            operation_id: cancel_id.clone(),
            target_operation_id: prompt_id,
            target: target.clone(),
            generation: Some(generation.clone()),
            requested_by: actor.clone().into(),
            approver: approver.clone(),
        },
    )
    .await?;
    let mut release = tokio::net::UnixStream::connect(root.path().join("exit.sock")).await?;
    use tokio::io::AsyncWriteExt as _;
    release.write_all(b"c").await?;
    let events = watch_turn(&mut observer, "turnEnded", initial_events).await?;
    assert_interaction_settles_before_turn_end(&events)?;
    let settled = client.list_approvals_with_options(false).await?;
    if !settled.approvals.iter().any(|approval| {
        approval.request_id == request_id && approval.reason.as_deref() == Some("turnCancelled")
    }) {
        return Err(format!(
            "cancelled approval not visible in detailed list: {:?}",
            settled.approvals
        )
        .into());
    }
    let state = listed_state(&client, &target).await?;
    if state != ProviderSessionState::Idle {
        return Err(format!("cancelled Session remained in {state:?}").into());
    }
    let (peer_prompt, peer_task) = prompt(&client, &peer, &actor, &approver, &generation)?;
    peer_task.await??;
    let peer_result = client
        .wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
            operation_id: peer_prompt,
            timeout_seconds: PositiveSeconds::try_from(3)?,
        })
        .await?;
    if !matches!(
        peer_result.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted { .. }
        }
    ) {
        return Err(format!(
            "peer Session failed after explicit cancel: {:?}",
            peer_result.output
        )
        .into());
    }
    runtime.shutdown().await?;
    Ok(())
}
