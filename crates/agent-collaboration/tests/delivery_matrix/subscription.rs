//! Subscription producer and recipient-observed stored-range proof for the delivery matrix.
use super::{ProofContext, ProofResult, SUBSCRIPTION_NOTICE_LABEL};
use collaboration_client::board::{
    BoardCreateRequest, BoardId, Description, Identity, MessageId, MessagePostRequest,
    MessageReferences, ParticipantRole, Placement, ProjectCreateRequest, ProjectId, ResourceName,
    SubscriptionMode, SubscriptionPolicyPatch, SubscriptionScope, SubscriptionTimingPatch,
    ThreadCreateRequest, ThreadJoinRequest, TopicCreateRequest, TopicId,
};
use collaboration_client::protocol::{
    PushKind, PushRecordShowParams, SessionRef, ThreadSubscribeRequest,
};
use serde_json::json;

pub(super) async fn verify_subscription_notice(
    proof: &mut ProofContext,
    target: &SessionRef,
    notice: &str,
    body_marker: &str,
) -> ProofResult<()> {
    if !notice.starts_with(SUBSCRIPTION_NOTICE_LABEL)
        || notice.lines().count() != 1
        || notice.contains(body_marker)
        || notice.contains("Self-declared sender:")
    {
        return Err("Subscription input was not a body-free neutral line".into());
    }
    let reference = notice
        .rsplit(" · ")
        .next()
        .ok_or("subscription link missing")?;
    let link = collaboration_client::protocol::RouterLink::parse(reference)?;
    let shown = proof
        .client
        .router_show(PushRecordShowParams {
            caller: target.clone(),
            reference: link.to_string(),
        })
        .await?;
    if shown.record.kind != PushKind::SubscriptionActivity
        || shown.record.target != *target
        || !shown
            .activity_ranges
            .iter()
            .flat_map(|range| &range.messages)
            .any(|message| message.text.as_str() == body_marker)
    {
        return Err("Subscription link did not expand the target's posted activity".into());
    }
    proof.record("subscriptionNotice", json!({"target":target,"pushId":shown.record.push_id,"line":notice,"rangeCount":shown.activity_ranges.len()}))?;
    Ok(())
}

pub(super) async fn board_subscription_push(
    proof: &mut ProofContext,
    sender: &SessionRef,
    codex: &SessionRef,
    peer: &SessionRef,
    codex_marker: &str,
    peer_marker: &str,
) -> ProofResult<()> {
    board_subscription_push_targets(proof, sender, &[(codex, codex_marker), (peer, peer_marker)])
        .await
}

pub(super) async fn board_subscription_push_targets(
    proof: &mut ProofContext,
    sender: &SessionRef,
    targets: &[(&SessionRef, &str)],
) -> ProofResult<()> {
    let actor: Identity = serde_json::from_value(json!({"kind":"session","session":sender}))?;
    let project_id = ProjectId::generate();
    let board_id = BoardId::generate();
    let topic_id = TopicId::generate();
    proof
        .client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from(format!("Delivery matrix {}", project_id.as_str()))?,
            description: Description::try_from("Disposable recipient delivery proof".to_owned())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    proof
        .client
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id,
            name: ResourceName::try_from(format!("Delivery board {}", board_id.as_str()))?,
            description: Description::try_from("Thread subscription recipient proof".to_owned())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    proof
        .client
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id,
            name: ResourceName::try_from("Delivery".to_owned())?,
            description: Description::try_from("One marker burst".to_owned())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    let root_id = MessageId::generate();
    proof
        .client
        .board_thread_create(ThreadCreateRequest {
            message_id: root_id.clone(),
            topic_id,
            actor: actor.clone(),
            acting_for: None,
            text: collaboration_client::board::MessageText::try_from(
                "Delivery matrix root".to_owned(),
            )?,
            references: MessageReferences::try_from(Vec::new())?,
            role: Some(ParticipantRole::Orchestrator),
            watch: true,
        })
        .await?;
    for (target, _) in targets {
        let reader: Identity = serde_json::from_value(json!({"kind":"session","session":target}))?;
        proof
            .client
            .board_thread_join(ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root_id.clone(),
                actor: reader.clone(),
                role: ParticipantRole::Participant,
                watch: true,
                replace: None,
                note: None,
            })
            .await?;
        proof
            .client
            .board_thread_subscribe(ThreadSubscribeRequest {
                actor: reader,
                scope: SubscriptionScope::thread(root_id.clone()),
                policy: SubscriptionPolicyPatch {
                    mode: Some(SubscriptionMode::Deliver),
                    timing: SubscriptionTimingPatch {
                        quiet_seconds: Some(1),
                        cap_seconds: Some(10),
                    },
                    ..SubscriptionPolicyPatch::default()
                },
            })
            .await?;
    }
    for (_, marker) in targets {
        proof
            .client
            .board_message_post(MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Thread {
                    root_message_id: root_id.clone(),
                },
                actor: actor.clone(),
                acting_for: None,
                text: collaboration_client::board::MessageText::try_from((*marker).to_owned())?,
                references: MessageReferences::try_from(Vec::new())?,
            })
            .await?;
    }
    Ok(())
}
