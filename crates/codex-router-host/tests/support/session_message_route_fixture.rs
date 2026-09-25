//! ACP, peer, wake, listen, and approval fixtures for Host composition proof.
use collaboration_client::{ControlClient, MessageSendRequest, PublicMessageContent};
use collaboration_protocol::{
    ApprovalDecideParams, ApprovalDecision, ConversationCreateRequest,
    ConversationOperationSettlement, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, ConversationPromptRequest, DeliveryDisposition,
    DeliveryEvidence, DeliveryOutcome, DeliveryShowRequest, EndpointRef, MessageContent,
    MessageDelivery, MessageText, OperationId, PositiveSeconds, ProviderRequestedPolicy,
    ProviderWorkingDirectory, RouterAccess, SessionId, SessionRef, WakeSendRequest,
    WakeShowRequest,
};
use message_board::{
    BoardCreateRequest, BoardId, Description, HumanId, Identity, MessageId, MessagePostRequest,
    MessageReferences, ParticipantRole, Placement, ProjectCreateRequest, ProjectId, ResourceName,
    ThreadJoinRequest, ThreadListenDelivery, ThreadListenMode, ThreadListenRequest,
    ThreadListenSelection, TopicCreateRequest, TopicId,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::{os::unix::fs::PermissionsExt as _, path::Path};

pub(super) fn provider_fixture(path: &Path) {
    std::fs::write(
        path,
        r#"#!/usr/bin/python3
import json,sys
session_id=sys.argv[1]
load_marker=sys.argv[2]
prompt_log=sys.argv[3]
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'agentInfo':{'name':'delivery-fixture','version':'1'},'_meta':{'steering':{'supported':True}}}})); sys.stdout.flush()
for line in sys.stdin:
 request=json.loads(line)
 method=request['method']
 if method=='session/new':
  result={'sessionId':session_id}
 elif method=='session/load':
  open(load_marker,'a').write('load\n')
  result={}
 elif method=='_session/steering':
  result={'outcome':'promptRequired'}
 elif method=='session/prompt':
  open(prompt_log,'a').write(json.dumps(request)+'\n')
  if 'request permission' in request['params']['prompt'][0]['text']:
   print(json.dumps({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':session_id,'toolCall':{'toolCallId':'permission-tool'},'options':[{'optionId':'allow-once','name':'Allow once','kind':'allow_once'},{'optionId':'deny-once','name':'Deny once','kind':'reject_once'}]}})); sys.stdout.flush()
   decision=json.loads(sys.stdin.readline())
   assert decision['id']==91
   assert decision['result']['outcome']['outcome']=='selected'
  result={'stopReason':'end_turn'}
 else:
  raise RuntimeError(method)
 print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result})); sys.stdout.flush()
"#,
    )
    .expect("provider fixture");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .expect("provider executable");
}

pub(super) fn message(target: SessionRef) -> MessageSendRequest {
    MessageSendRequest {
        target,
        message: PublicMessageContent::HumanUser {
            text: MessageText::try_from("routed message".to_owned()).expect("message"),
        },
        delivery: MessageDelivery::Auto,
        generation_guard: None,
        correlation: None,
    }
}

pub(super) async fn wait_for_prompt_text(path: &Path, text: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if std::fs::read_to_string(path).is_ok_and(|log| log.contains(text)) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "provider prompt log deadline; log: {}",
            std::fs::read_to_string(path).unwrap_or_default()
        )
    });
}

pub(super) async fn post_thread_activity_for_sessions(
    client: &mut ControlClient,
    readers: [SessionRef; 2],
) {
    let owner = Identity::Human {
        human_id: HumanId::try_from("route-test-owner".to_owned()).expect("owner"),
    };
    let project_id = ProjectId::generate();
    let board_id = BoardId::generate();
    let topic_id = TopicId::generate();
    client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("Route project".to_owned()).expect("project"),
            description: Description::try_from(String::new()).expect("description"),
            actor: owner.clone(),
            acting_for: None,
        })
        .await
        .expect("project");
    client
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id,
            name: ResourceName::try_from("Route board".to_owned()).expect("board"),
            description: Description::try_from(String::new()).expect("description"),
            actor: owner.clone(),
            acting_for: None,
        })
        .await
        .expect("board");
    client
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id,
            name: ResourceName::try_from("Route topic".to_owned()).expect("topic"),
            description: Description::try_from(String::new()).expect("description"),
            actor: owner.clone(),
            acting_for: None,
        })
        .await
        .expect("topic");
    let root = client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic { topic_id },
            actor: owner.clone(),
            acting_for: None,
            text: message_board::MessageText::try_from("Route root".to_owned()).expect("root text"),
            references: MessageReferences::try_from(Vec::new()).expect("references"),
        })
        .await
        .expect("root post");
    let root_message_id = root.message.message_id;
    for target in readers {
        let reader: Identity = serde_json::from_value(json!({
            "kind":"session", "session": target
        }))
        .expect("session reader");
        client
            .board_thread_join(ThreadJoinRequest {
                root_message_id: root_message_id.clone(),
                actor: reader.clone(),
                role: ParticipantRole::Participant,
                watch: true,
                replace: None,
                note: None,
            })
            .await
            .expect("reader joined");
        client
            .board_thread_listen(ThreadListenRequest {
                reader,
                selection: ThreadListenSelection::Roots {
                    root_message_ids: vec![root_message_id.clone()],
                },
                mode: ThreadListenMode::Once {
                    max_wait_seconds: 25 * 60,
                },
                from_activity_sequence: None,
                acknowledge: false,
                delivery: ThreadListenDelivery::Session,
            })
            .await
            .expect("session listen");
    }
    client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread { root_message_id },
            actor: owner,
            acting_for: None,
            text: message_board::MessageText::try_from("Route reply".to_owned())
                .expect("reply text"),
            references: MessageReferences::try_from(Vec::new()).expect("references"),
        })
        .await
        .expect("reply post");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(5 * 60 + 2)).await;
    tokio::time::resume();
}

pub(super) fn publish_peer(registry: &Path, session_id: &SessionId, protocol: u64, socket: &Path) {
    let process_id = std::process::id();
    std::fs::write(
        registry.join(format!("{process_id}.json")),
        json!({
            "pid": process_id,
            "sessionId": String::from(session_id.clone()),
            "status": "busy",
            "peerProtocol": protocol,
            "messagingSocketPath": socket,
        })
        .to_string(),
    )
    .expect("peer registry record");
    let digest = Sha256::digest(socket.to_str().expect("socket path").as_bytes());
    let key = registry.join(format!("{process_id}.{digest:x}.key"));
    std::fs::write(
        &key,
        json!({"peerToken":"0123456789abcdef0123456789abcdef"}).to_string(),
    )
    .expect("peer key");
    std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600))
        .expect("private peer key");
}

pub(super) async fn create_provider_target(
    client: &mut ControlClient,
    endpoint: EndpointRef,
    actor: SessionRef,
    working_directory: &Path,
) -> SessionRef {
    let generation = client
        .list_endpoints()
        .await
        .expect("catalog")
        .endpoints
        .into_iter()
        .find(|entry| entry.endpoint == endpoint)
        .expect("provider endpoint")
        .channels
        .into_iter()
        .find_map(|channel| match channel {
            collaboration_protocol::ChannelDescription::ExternalProvider {
                binding_generation,
                ..
            } => Some(binding_generation),
            _ => None,
        })
        .expect("binding generation");
    let operation_id = OperationId::generate();
    client
        .create_provider_conversation(ConversationCreateRequest {
            operation_id: operation_id.clone(),
            endpoint,
            generation: Some(collaboration_protocol::CodexGeneration {
                service_epoch: client.identity().service_epoch.clone(),
                generation,
            }),
            working_directory: ProviderWorkingDirectory::try_from(
                working_directory.display().to_string(),
            )
            .expect("working directory"),
            created_by: actor.clone(),
            approver: actor,
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
        })
        .await
        .expect("create admitted");
    let waited = client
        .wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
            operation_id,
            timeout_seconds: PositiveSeconds::try_from(3).expect("timeout"),
        })
        .await
        .expect("create settled");
    match waited.output {
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { target, .. },
        } => target,
        other => panic!("provider create did not settle: {other:?}"),
    }
}

pub(super) async fn send_and_wait_wake(client: &mut ControlClient, target: SessionRef) {
    let request: WakeSendRequest = serde_json::from_value(json!({
        "operationId": OperationId::generate(),
        "message": {
            "target": target,
            "content": {"kind":"humanUser","text":"wake through provider"},
            "delivery": "auto",
            "generationGuard": null
        },
        "timing": {"kind":"at","at":"2026-01-01T00:00:00.000Z"},
        "expiry": {"kind":"none"}
    }))
    .expect("wake request");
    let wake = client.send_wakeup(request).await.expect("wake accepted");
    let delivery_id = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let snapshot = client
                .read_wakeup(WakeShowRequest {
                    wakeup_id: wake.definition.wakeup_id.clone(),
                })
                .await
                .expect("wake inspection");
            if let Some(delivery_id) = snapshot.pending_delivery_id {
                break delivery_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("wake fire deadline");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let delivery = client
                .read_delivery(DeliveryShowRequest {
                    delivery_id: delivery_id.clone(),
                })
                .await
                .expect("delivery inspection");
            if matches!(delivery.disposition, DeliveryDisposition::Accepted) {
                assert!(matches!(delivery.evidence,
                    DeliveryEvidence::Accepted { receipt, .. }
                    if receipt.outcome == DeliveryOutcome::Started
                        && receipt.reachability == Some(collaboration_protocol::SessionReachability::ProviderAcp)));
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("wake delivery deadline");
}

pub(super) async fn prompt_and_approve_from_peer_provider(
    client: &mut ControlClient,
    requester: SessionRef,
    approver: SessionRef,
    approver_prompt_log: &Path,
) {
    let inventory = client.list_endpoints().await.expect("catalog");
    let generation_number = inventory
        .endpoints
        .iter()
        .find(|entry| entry.endpoint == requester.endpoint)
        .expect("requester endpoint")
        .channels
        .iter()
        .find_map(|channel| match channel {
            collaboration_protocol::ChannelDescription::ExternalProvider {
                binding_generation,
                ..
            } => Some(*binding_generation),
            _ => None,
        })
        .expect("requester generation");
    let operation_id = OperationId::generate();
    client
        .prompt_provider_conversation(ConversationPromptRequest {
            operation_id: operation_id.clone(),
            target: requester.clone(),
            generation: Some(collaboration_protocol::CodexGeneration {
                service_epoch: inventory.service_epoch,
                generation: generation_number,
            }),
            requested_by: requester,
            approver: approver.clone(),
            prompt: MessageContent::HumanUser {
                text: MessageText::try_from("request permission".to_owned())
                    .expect("permission prompt"),
            },
        })
        .await
        .expect("permission prompt admitted");
    wait_for_prompt_text(approver_prompt_log, "approval-").await;
    let pending = client
        .list_pending_approvals(true)
        .await
        .expect("pending approvals")
        .approvals
        .into_iter()
        .find(|record| record.approver == approver)
        .expect("approval for other provider");
    client
        .decide_approval(ApprovalDecideParams {
            request_id: pending.request_id,
            decision: ApprovalDecision::Allow,
            actor: approver,
        })
        .await
        .expect("approval decision");
    let settled = client
        .wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
            operation_id,
            timeout_seconds: PositiveSeconds::try_from(3).expect("timeout"),
        })
        .await
        .expect("permission prompt settled");
    assert!(matches!(
        settled.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted { .. }
        }
    ));
}
