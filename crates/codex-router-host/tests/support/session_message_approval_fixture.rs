//! Real approval-push and provider-decision observations through the collaboration API.
use super::{provider_prompt_contains, spawn_provider_prompt, wait_for_prompt_text};
use collaboration_client::CollaborationClient;
use collaboration_protocol::{
    ApprovalDecideParams, ConversationOperationSettlement, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, ConversationPromptRequest, MessageContent, MessageText,
    OperationId, PositiveSeconds, SessionRef,
};
use serde_json::json;
use std::path::Path;

pub(crate) async fn prompt_and_approve_from_peer_provider(
    client: &CollaborationClient,
    requester: SessionRef,
    approver: SessionRef,
    approver_prompt_log: &Path,
) -> collaboration_protocol::PushRecordShowResult {
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
    // The prompt settles only after the approval below, so it runs in the background.
    let prompt = spawn_provider_prompt(
        client,
        ConversationPromptRequest {
            input_id: None,
            operation_id: operation_id.clone(),
            target: requester.clone(),
            generation: Some(collaboration_protocol::CodexGeneration {
                service_epoch: inventory.service_epoch,
                generation: generation_number,
            }),
            requested_by: (requester).into(),
            approver: (approver.clone()).into(),
            prompt: MessageContent::HumanUser {
                text: MessageText::try_from("request permission".to_owned())
                    .expect("permission prompt"),
            },
        },
    );
    let notice_line = wait_for_prompt_text(approver_prompt_log, "❓ Router approval").await;
    assert_eq!(
        notice_line.lines().count(),
        1,
        "single neutral approval notice"
    );
    assert_eq!(
        collaboration_protocol::parse_push_line_header(&notice_line)
            .expect("approval push header")
            .kind,
        collaboration_protocol::PushKind::Approval,
    );
    let notice_link = notice_line.rsplit_once(" · ").expect("notice link").1;
    let locator =
        collaboration_protocol::RouterLink::parse(notice_link).expect("validated stored push link");
    let stored_notice = client
        .router_show(collaboration_protocol::PushRecordShowParams {
            caller: approver.clone(),
            reference: notice_link.to_owned(),
        })
        .await
        .expect("Host-composed stored approval push");
    assert_eq!(&stored_notice.record.push_id, locator.push_id());
    assert_eq!(stored_notice.link, notice_link);
    assert_eq!(
        stored_notice.record.kind,
        collaboration_protocol::PushKind::Approval
    );
    assert_eq!(stored_notice.record.target, approver);
    assert_eq!(
        stored_notice.record.origin,
        collaboration_protocol::PushOrigin::Router(collaboration_protocol::PushKind::Approval)
    );
    let pending = client
        .list_approvals_with_options(true)
        .await
        .expect("pending approvals")
        .approvals
        .into_iter()
        .find(|record| {
            serde_json::to_value(&record.approver).ok()
                == Some(json!({"kind":"session","session":approver.clone()}))
        })
        .expect("approval for other provider");
    assert!(
        pending
            .options
            .iter()
            .any(|option| option.option_id == "allow-once")
    );
    let body = stored_notice
        .record
        .body
        .as_deref()
        .expect("stored approval details");
    let origin_reference = collaboration_protocol::RouterOriginRef::parse_canonical(
        stored_notice
            .record
            .origin_router_ref
            .as_deref()
            .expect("interaction origin reference"),
    )
    .expect("typed interaction origin");
    assert!(matches!(origin_reference,
        collaboration_protocol::RouterOriginRef::Interaction { interaction_id, .. }
            if interaction_id.as_str() == pending.request_id));
    assert!(body.contains(&format!("Request ID: {}", pending.request_id)));
    assert!(body.contains("permission-tool"));
    assert!(body.contains("allow-once"));
    assert!(body.contains("deny-once"));
    client
        .decide_approval(ApprovalDecideParams {
            request_id: pending.request_id,
            decision: None,
            option_id: Some("allow-once".to_owned()),
            acknowledge_persistent: false,
            note: None,
            actor: serde_json::from_value(json!({"kind":"session","session":approver.clone()}))
                .expect("approver identity"),
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
    prompt
        .await
        .expect("permission prompt task")
        .expect("permission prompt settled through the API");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let log = std::fs::read_to_string(approver_prompt_log).unwrap_or_default();
            let notice_id = log
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find(|entry| provider_prompt_contains(entry, "❓ Router approval"))
                .and_then(|entry| entry["id"].as_str().map(str::to_owned));
            if notice_id
                .is_some_and(|id| log.contains(&format!("\"fixturePromptCompleted\": \"{id}\"")))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approver notice turn settled before later board activity");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if client
                .inspect_provider_session(collaboration_protocol::ProviderSessionInspectRequest {
                    target: approver.clone(),
                })
                .await
                .is_ok_and(|session| {
                    session.state == collaboration_protocol::ProviderSessionState::Idle
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approver Session idle before later board activity");
    stored_notice
}
