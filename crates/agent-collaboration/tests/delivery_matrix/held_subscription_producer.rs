//! Two-root producer for the held-subscription acceptance cell.
use super::ProofResult;
use collaboration_client::CollaborationClient;
use collaboration_client::board::{
    BoardCreateRequest, BoardId, Description, Identity, MessageId, MessagePostRequest,
    MessageReferences, ParticipantRole, Placement, ProjectCreateRequest, ProjectId, ResourceName,
    SubscriptionMode, SubscriptionPolicyPatch, SubscriptionScope, SubscriptionTimingPatch,
    ThreadCreateRequest, ThreadJoinRequest, TopicCreateRequest, TopicId, WhenIdle,
};
use collaboration_client::protocol::{SessionRef, ThreadSubscribeRequest};
use serde_json::json;

pub(super) async fn board_two_thread_subscription_push(
    client: &CollaborationClient,
    sender: &SessionRef,
    target: &SessionRef,
    markers: [&str; 2],
    resources: &mut super::owned_resources::OwnedMatrixResources,
) -> ProofResult<Vec<MessageId>> {
    let actor: Identity = serde_json::from_value(json!({"kind":"session","session":sender}))?;
    let project_id = ProjectId::generate();
    client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from(format!("Held matrix {}", project_id.as_str()))?,
            description: Description::try_from("Two-root held subscription proof".to_owned())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;

    let reader: Identity = serde_json::from_value(json!({"kind":"session","session":target}))?;
    let mut roots = Vec::with_capacity(markers.len());
    for (index, _) in markers.iter().enumerate() {
        // The existing per-actor cooldown is per board; do not bypass it or sleep.
        let board_id = BoardId::generate();
        let topic_id = TopicId::generate();
        client
            .board_create(BoardCreateRequest {
                board_id: board_id.clone(),
                project_id: project_id.clone(),
                name: ResourceName::try_from(format!("Held board {}", board_id.as_str()))?,
                description: Description::try_from("Held batch recipient proof".to_owned())?,
                actor: actor.clone(),
                acting_for: None,
            })
            .await?;
        client
            .board_topic_create(TopicCreateRequest {
                topic_id: topic_id.clone(),
                board_id,
                name: ResourceName::try_from("Held batches".to_owned())?,
                description: Description::try_from("Two subscribed roots".to_owned())?,
                actor: actor.clone(),
                acting_for: None,
            })
            .await?;
        let root_id = MessageId::generate();
        resources.register_actor_root(actor.clone(), root_id.clone())?;
        client
            .board_thread_create(ThreadCreateRequest {
                message_id: root_id.clone(),
                topic_id: topic_id.clone(),
                actor: actor.clone(),
                acting_for: None,
                text: collaboration_client::board::MessageText::try_from(format!(
                    "Held matrix root {index}"
                ))?,
                references: MessageReferences::try_from(Vec::new())?,
                role: Some(ParticipantRole::Orchestrator),
                watch: true,
            })
            .await?;
        resources.register_actor_root(reader.clone(), root_id.clone())?;
        client
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
        client
            .board_thread_subscribe(ThreadSubscribeRequest {
                actor: reader.clone(),
                scope: SubscriptionScope::thread(root_id.clone()),
                policy: SubscriptionPolicyPatch {
                    mode: Some(SubscriptionMode::Deliver),
                    when_idle: Some(WhenIdle::Hold),
                    timing: SubscriptionTimingPatch {
                        quiet_seconds: Some(1),
                        cap_seconds: Some(10),
                    },
                    ..SubscriptionPolicyPatch::default()
                },
            })
            .await?;
        roots.push(root_id);
    }
    for (root_message_id, marker) in roots.iter().cloned().zip(markers) {
        client
            .board_message_post(MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Thread { root_message_id },
                actor: actor.clone(),
                acting_for: None,
                text: collaboration_client::board::MessageText::try_from(marker.to_owned())?,
                references: MessageReferences::try_from(Vec::new())?,
            })
            .await?;
    }
    Ok(roots)
}
