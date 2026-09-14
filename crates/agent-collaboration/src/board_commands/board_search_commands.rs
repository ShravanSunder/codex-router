use super::board_arguments::{CommonArguments, MessageScopeKind, PageArguments};
use super::board_preparation::{CommandContext, PreparedBoardCommand};
use super::board_value_parsing::{
    command_context, parse_bounded_text, parse_uuid_v7, prepare_page_request,
};
use clap::{Args, ValueEnum};
use collaboration_client::board::*;

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum DiscoveryScopeKind {
    AllProjects,
    Project,
    Board,
}
#[derive(Clone, Copy, ValueEnum)]
pub(super) enum DiscoveryResultKind {
    All,
    Project,
    Board,
    Topic,
}
#[derive(Clone, Copy, ValueEnum)]
pub(super) enum MessageResultKind {
    Both,
    TopLevel,
    Thread,
}

#[derive(Args)]
pub(super) struct DiscoverySearchArguments {
    /// Literal substring; 1 to 256 UTF-8 bytes after trimming. ASCII case-insensitive.
    #[arg(long)]
    pub query: String,
    #[arg(long, value_enum, default_value = "all-projects")]
    pub scope: DiscoveryScopeKind,
    #[arg(long)]
    pub project_id: Option<String>,
    #[arg(long)]
    pub board_id: Option<String>,
    #[arg(long, value_enum, default_value = "all")]
    pub kind: DiscoveryResultKind,
    #[arg(long)]
    pub include_archived: bool,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct MessageSearchArguments {
    /// Literal substring; 1 to 256 UTF-8 bytes after trimming. ASCII case-insensitive.
    #[arg(long)]
    pub query: String,
    #[arg(long, value_enum, default_value = "all-projects")]
    pub scope: MessageScopeKind,
    #[arg(long)]
    pub project_id: Option<String>,
    #[arg(long)]
    pub board_id: Option<String>,
    #[arg(long)]
    pub topic_id: Option<String>,
    #[arg(long)]
    pub root_message_id: Option<String>,
    #[arg(long, value_enum, default_value = "both")]
    pub kind: MessageResultKind,
    #[arg(long)]
    pub include_archived: bool,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

pub(super) fn prepare_discovery_search(
    arguments: DiscoverySearchArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    let scope = match (arguments.scope, arguments.project_id, arguments.board_id) {
        (DiscoveryScopeKind::AllProjects, None, None) => DiscoveryScope::AllProjects,
        (DiscoveryScopeKind::Project, Some(value), None) => DiscoveryScope::Project {
            project_id: parse_uuid_v7(value, "--project-id")?,
        },
        (DiscoveryScopeKind::Board, None, Some(value)) => DiscoveryScope::Board {
            board_id: parse_uuid_v7(value, "--board-id")?,
        },
        _ => {
            return Err(
                "--scope requires exactly its matching ID (or none for all-projects)".into(),
            );
        }
    };
    let kind = match arguments.kind {
        DiscoveryResultKind::All => DiscoveryKind::All,
        DiscoveryResultKind::Project => DiscoveryKind::Project,
        DiscoveryResultKind::Board => DiscoveryKind::Board,
        DiscoveryResultKind::Topic => DiscoveryKind::Topic,
    };
    if matches!(scope, DiscoveryScope::Board { .. }) && kind == DiscoveryKind::Project {
        return Err("--kind project cannot be used within a board scope".into());
    }
    Ok((
        PreparedBoardCommand::DiscoverySearch(DiscoverySearchRequest {
            query: parse_bounded_text(arguments.query, "--query")?,
            scope,
            kind,
            include_archived: arguments.include_archived,
            page: prepare_page_request(arguments.page)?,
        }),
        command_context(arguments.common),
    ))
}

pub(super) fn prepare_message_search(
    arguments: MessageSearchArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    let scope = match (
        arguments.scope,
        arguments.project_id,
        arguments.board_id,
        arguments.topic_id,
        arguments.root_message_id,
    ) {
        (MessageScopeKind::AllProjects, None, None, None, None) => MessageListScope::AllProjects,
        (MessageScopeKind::Project, Some(value), None, None, None) => MessageListScope::Project {
            project_id: parse_uuid_v7(value, "--project-id")?,
        },
        (MessageScopeKind::Board, None, Some(value), None, None) => MessageListScope::Board {
            board_id: parse_uuid_v7(value, "--board-id")?,
        },
        (MessageScopeKind::Topic, None, None, Some(value), None) => MessageListScope::Topic {
            topic_id: parse_uuid_v7(value, "--topic-id")?,
        },
        (MessageScopeKind::Thread, None, None, None, Some(value)) => MessageListScope::Thread {
            root_message_id: parse_uuid_v7(value, "--root-message-id")?,
        },
        _ => {
            return Err(
                "--scope requires exactly its matching ID (or none for all-projects)".into(),
            );
        }
    };
    let kind = match arguments.kind {
        MessageResultKind::Both => SearchMessageKind::Both,
        MessageResultKind::TopLevel => SearchMessageKind::TopLevel,
        MessageResultKind::Thread => SearchMessageKind::Thread,
    };
    if matches!(scope, MessageListScope::Thread { .. }) && kind == SearchMessageKind::TopLevel {
        return Err("--kind top-level cannot be used within a thread scope".into());
    }
    Ok((
        PreparedBoardCommand::MessageSearch(MessageSearchRequest {
            query: parse_bounded_text(arguments.query, "--query")?,
            scope,
            kind,
            include_archived: arguments.include_archived,
            page: prepare_page_request(arguments.page)?,
        }),
        command_context(arguments.common),
    ))
}
