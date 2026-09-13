use super::{
    OWNER_HUMAN_ID, PRIMARY_BOARD, PRIMARY_PROJECT, PRIMARY_TOPIC, REPOSITORY_ORIGIN,
    SECONDARY_PROJECT,
};
use crate::proof_context::{ProofContext, ProofResult};
use collaboration_client::board::*;

pub(super) struct NamedResources {
    pub primary_project: Project,
    pub primary_board: Board,
    pub primary_topic: Topic,
}

pub(super) struct DiscussionResources {
    pub primary_root: MessageId,
    pub cross_project_reply: MessageId,
}

pub(super) async fn discover_named_resources(
    proof: &mut ProofContext,
) -> ProofResult<NamedResources> {
    let projects = proof
        .client
        .board_project_list(ProjectListRequest {
            repository: Some(RepositoryRef::Origin {
                normalized_origin: NormalizedOrigin::try_from(REPOSITORY_ORIGIN.to_owned())?,
            }),
            page: PageRequest::default(),
        })
        .await?
        .page
        .records;
    let primary_project = projects
        .into_iter()
        .find(|project| project.name.as_str() == PRIMARY_PROJECT)
        .ok_or("setup operator did not create/discover the authorized primary project")?;
    let boards = proof
        .client
        .board_list(BoardListRequest {
            project_id: Some(primary_project.project_id.clone()),
            include_archived: true,
            page: PageRequest::default(),
        })
        .await?
        .page
        .records;
    let primary_board = boards
        .into_iter()
        .find(|board| board.name.as_str() == PRIMARY_BOARD)
        .ok_or("setup operator did not create the authorized primary board")?;
    let topics = proof
        .client
        .board_topic_list(TopicListRequest {
            board_id: primary_board.board_id.clone(),
            page: PageRequest::default(),
        })
        .await?
        .page
        .records;
    let primary_topic = topics
        .into_iter()
        .find(|topic| topic.name.as_str() == PRIMARY_TOPIC)
        .ok_or("setup operator did not create the authorized primary topic")?;
    Ok(NamedResources {
        primary_project,
        primary_board,
        primary_topic,
    })
}

pub(super) async fn grade_discussion(
    proof: &mut ProofContext,
    resources: &NamedResources,
    discussion_identity: &Identity,
) -> ProofResult<DiscussionResources> {
    let primary_messages = proof
        .client
        .board_message_list(MessageListRequest {
            scope: MessageListScope::Topic {
                topic_id: resources.primary_topic.topic_id.clone(),
            },
            selection: MessageSelection::Latest,
            page: PageRequest::default(),
        })
        .await?
        .page
        .records;
    let primary_root = primary_messages
        .into_iter()
        .find(|message| {
            message.actor == *discussion_identity
                && message.text.as_str().contains("Migration readiness")
        })
        .ok_or("discussion operator did not create the primary Migration readiness root")?;
    let projects = proof
        .client
        .board_project_list(ProjectListRequest {
            repository: None,
            page: PageRequest {
                limit: PageLimit::try_from(100)?,
                cursor: None,
            },
        })
        .await?
        .page
        .records;
    let secondary = projects
        .into_iter()
        .find(|project| project.name.as_str() == SECONDARY_PROJECT)
        .ok_or("discussion operator did not create the authorized secondary project")?;
    let secondary_board = proof
        .client
        .board_list(BoardListRequest {
            project_id: Some(secondary.project_id),
            include_archived: true,
            page: PageRequest::default(),
        })
        .await?
        .page
        .records
        .into_iter()
        .next()
        .ok_or("secondary project has no board")?;
    let secondary_messages = proof
        .client
        .board_message_list(MessageListRequest {
            scope: MessageListScope::Board {
                board_id: secondary_board.board_id,
            },
            selection: MessageSelection::Latest,
            page: PageRequest {
                limit: PageLimit::try_from(100)?,
                cursor: None,
            },
        })
        .await?
        .page
        .records;
    let owner = ActingForIdentity::Human {
        human_id: HumanId::try_from(OWNER_HUMAN_ID.to_owned())?,
    };
    let reply = secondary_messages.into_iter().find(|message| {
        message.actor == *discussion_identity
            && message.acting_for == Some(owner.clone())
            && matches!(message.placement, Placement::Thread { .. })
            && message.text.as_str().contains("42")
            && message.references.iter().any(|reference| reference == &ReferenceTarget::Message { message_id: primary_root.message_id.clone() })
            && message.references.iter().any(|reference| reference == &ReferenceTarget::Thread { root_message_id: primary_root.message_id.clone() })
    }).ok_or("cross-project reply had wrong actor, acting-for, placement, references, or fixture result")?;
    let watch = proof
        .client
        .board_thread_show(ThreadShowRequest {
            root_message_id: reply.placement.clone().thread_root()?,
            reader: Some(discussion_identity.clone()),
        })
        .await?;
    if watch.watch_status.is_none_or(|status| !status.watching) {
        return Err("discussion operator's thread post did not persist its automatic watch".into());
    }
    Ok(DiscussionResources {
        primary_root: primary_root.message_id,
        cross_project_reply: reply.message_id,
    })
}

pub(super) async fn grade_inbox(
    proof: &mut ProofContext,
    resources: &NamedResources,
    discussions: &DiscussionResources,
    inbox_identity: &Identity,
) -> ProofResult<()> {
    let watch = proof
        .client
        .board_thread_show(ThreadShowRequest {
            root_message_id: discussions.primary_root.clone(),
            reader: Some(inbox_identity.clone()),
        })
        .await?;
    if watch.watch_status.is_none_or(|status| !status.watching) {
        return Err("inbox operator did not end with the primary thread watched".into());
    }
    let inbox = proof
        .client
        .board_inbox_fetch(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: resources.primary_project.project_id.clone(),
            },
            read_mode: InboxReadMode::Unread,
            reader: inbox_identity.clone(),
            page: PageRequest::default(),
        })
        .await?;
    if !inbox.page.records.is_empty() {
        return Err("inbox operator left seeded watched-thread activity unacknowledged".into());
    }
    Ok(())
}

pub(super) async fn grade_lifecycle(
    proof: &mut ProofContext,
    resources: &NamedResources,
    discussions: &DiscussionResources,
) -> ProofResult<()> {
    let board = proof
        .client
        .board_show(BoardShowRequest {
            board_id: resources.primary_board.board_id.clone(),
        })
        .await?
        .board;
    if board.state != BoardState::Archived {
        return Err("lifecycle operator did not archive the primary board".into());
    }
    let repositories = proof
        .client
        .board_repository_list(RepositoryListRequest {
            project_id: resources.primary_project.project_id.clone(),
            page: PageRequest::default(),
        })
        .await?
        .page
        .records;
    if !repositories.is_empty() {
        return Err("lifecycle operator did not detach the primary repository association".into());
    }
    let thread = proof
        .client
        .board_thread_show(ThreadShowRequest {
            root_message_id: discussions.primary_root.clone(),
            reader: None,
        })
        .await?
        .thread;
    if thread.state != ThreadState::Unresolved {
        return Err(
            "lifecycle operator did not restore the thread to unresolved before archive".into(),
        );
    }
    proof
        .client
        .board_message_show(MessageShowRequest {
            message_id: discussions.primary_root.clone(),
        })
        .await?;
    Ok(())
}

trait PlacementThreadRoot {
    fn thread_root(self) -> ProofResult<MessageId>;
}

impl PlacementThreadRoot for Placement {
    fn thread_root(self) -> ProofResult<MessageId> {
        match self {
            Placement::Thread { root_message_id } => Ok(root_message_id),
            Placement::Topic { .. } => Err("expected thread placement".into()),
        }
    }
}
