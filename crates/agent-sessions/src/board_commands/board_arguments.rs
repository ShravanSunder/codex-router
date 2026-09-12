use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "agent-sessions board",
    bin_name = "agent-sessions board",
    about = "Coordinate durable project message boards through the selected Router service"
)]
pub(super) struct BoardArguments {
    #[command(subcommand)]
    pub command: BoardCommand,
}

#[derive(Subcommand)]
pub(super) enum BoardCommand {
    /// Create, inspect, update, or list projects.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Attach, detach, or list project repository associations.
    Repository {
        #[command(subcommand)]
        command: RepositoryCommand,
    },
    /// Create a board in a project. Obtain the project owner's permission before running this command.
    Create(BoardCreateArguments),
    /// Replace a board's name and description.
    Update(BoardUpdateArguments),
    /// Show one board.
    Show(BoardShowArguments),
    /// List boards, excluding archived boards unless requested.
    List(BoardListArguments),
    /// Archive a board and make its content read-only.
    Archive(BoardArchiveArguments),
    /// Create, update, or list topics.
    Topic {
        #[command(subcommand)]
        command: TopicCommand,
    },
    /// Post, show, or list immutable messages.
    Message {
        #[command(subcommand)]
        command: MessageCommand,
    },
    /// Inspect, change, watch, or list root-message threads.
    Thread {
        #[command(subcommand)]
        command: ThreadCommand,
    },
    /// Fetch or acknowledge personal unread activity.
    Inbox {
        #[command(subcommand)]
        command: InboxCommand,
    },
}

#[derive(Subcommand)]
pub(super) enum ProjectCommand {
    /// Create a project. Obtain the project owner's permission before running this command.
    Create(ProjectCreateArguments),
    /// Replace a project's name and description.
    Update(ProjectUpdateArguments),
    /// Show one project.
    Show(ProjectShowArguments),
    /// List projects, optionally associated with a repository.
    List(ProjectListArguments),
}

#[derive(Subcommand)]
pub(super) enum RepositoryCommand {
    /// Attach the repository discovered from a path or explicit origin.
    Attach(RepositoryMutationArguments),
    /// Detach the repository discovered from a path or explicit origin.
    Detach(RepositoryMutationArguments),
    /// List repositories associated with a project.
    List(RepositoryListArguments),
}

#[derive(Subcommand)]
pub(super) enum TopicCommand {
    /// Create a topic on an active board.
    Create(TopicCreateArguments),
    /// Replace a topic's name and description.
    Update(TopicUpdateArguments),
    /// List topics on a board.
    List(TopicListArguments),
}

#[derive(Subcommand)]
pub(super) enum MessageCommand {
    /// Post an immutable top-level or thread message.
    Post(MessagePostArguments),
    /// Show one message.
    Show(MessageShowArguments),
    /// List message history with an explicit scope and selection mode.
    List(MessageListArguments),
}

#[derive(Subcommand)]
pub(super) enum ThreadCommand {
    /// Show a root-message thread and optional reader watch status.
    Show(ThreadShowArguments),
    /// Mark a thread resolved.
    Resolve(ThreadMutationArguments),
    /// Mark a thread unresolved so thread messages can be posted again.
    Unresolve(ThreadMutationArguments),
    /// Watch future activity in a thread. Earlier history remains an explicit range.
    Watch(ThreadMutationArguments),
    /// Stop watching a thread. Existing message history remains readable.
    Unwatch(ThreadMutationArguments),
    /// List project threads and reader watch status.
    List(ThreadListArguments),
}

#[derive(Subcommand)]
pub(super) enum InboxCommand {
    /// Fetch unread activity without acknowledging it.
    Fetch(InboxFetchArguments),
    /// Acknowledge one topic or thread through an activity sequence.
    Acknowledge(InboxAcknowledgeArguments),
    /// List projects with personal inbox tracking or watches.
    Projects(InboxProjectsArguments),
}

#[derive(Args)]
pub(super) struct CommonArguments {
    /// Router communication service directory. Must be absolute.
    #[arg(long)]
    pub service_directory: Option<PathBuf>,
    /// Emit one compact JSON result or error object.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub(super) struct MutationIdentityArguments {
    /// Typed actor Identity JSON, for example {"kind":"human","humanId":"alice"}.
    #[arg(long)]
    pub actor: String,
    /// Optional human attribution JSON, for example {"kind":"human","humanId":"owner"}.
    #[arg(long)]
    pub acting_for: Option<String>,
}

#[derive(Args)]
pub(super) struct PageArguments {
    /// Maximum records, from 1 through 100.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: u32,
    /// Opaque cursor returned by the preceding page of the same query.
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Args)]
pub(super) struct RepositorySelectorArguments {
    /// Git remote origin to normalize into the repository identity.
    #[arg(long, conflicts_with = "repository_path")]
    pub repository_origin: Option<String>,
    /// Path inside a local Git repository; origin is preferred, otherwise its common directory is used.
    #[arg(long, value_name = "PATH", conflicts_with = "repository_origin")]
    pub repository_path: Option<PathBuf>,
}

#[derive(Args)]
pub(super) struct ProjectCreateArguments {
    /// Caller-chosen UUIDv7; generated and reported when omitted.
    #[arg(long)]
    pub project_id: Option<String>,
    #[arg(long)]
    pub name: String,
    #[arg(long, default_value = "")]
    pub description: String,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct ProjectUpdateArguments {
    #[arg(long)]
    pub project_id: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub description: String,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct ProjectShowArguments {
    #[arg(long)]
    pub project_id: String,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct ProjectListArguments {
    #[command(flatten)]
    pub repository: RepositorySelectorArguments,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct RepositoryMutationArguments {
    #[arg(long)]
    pub project_id: String,
    #[command(flatten)]
    pub repository: RepositorySelectorArguments,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct RepositoryListArguments {
    #[arg(long)]
    pub project_id: String,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct BoardCreateArguments {
    /// Caller-chosen UUIDv7; generated and reported when omitted.
    #[arg(long)]
    pub board_id: Option<String>,
    #[arg(long)]
    pub project_id: String,
    #[arg(long)]
    pub name: String,
    #[arg(long, default_value = "")]
    pub description: String,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct BoardUpdateArguments {
    #[arg(long)]
    pub board_id: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub description: String,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct BoardShowArguments {
    #[arg(long)]
    pub board_id: String,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct BoardListArguments {
    #[arg(long)]
    pub project_id: Option<String>,
    #[arg(long)]
    pub include_archived: bool,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct BoardArchiveArguments {
    #[arg(long)]
    pub board_id: String,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct TopicCreateArguments {
    /// Caller-chosen UUIDv7; generated and reported when omitted.
    #[arg(long)]
    pub topic_id: Option<String>,
    #[arg(long)]
    pub board_id: String,
    #[arg(long)]
    pub name: String,
    #[arg(long, default_value = "")]
    pub description: String,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct TopicUpdateArguments {
    #[arg(long)]
    pub topic_id: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub description: String,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct TopicListArguments {
    #[arg(long)]
    pub board_id: String,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum PlacementKind {
    Topic,
    Thread,
}

#[derive(Args)]
pub(super) struct MessagePostArguments {
    /// Caller-chosen UUIDv7; generated and reported when omitted.
    #[arg(long)]
    pub message_id: Option<String>,
    /// Whether this is a top-level topic message or a message in a root thread.
    #[arg(long, value_enum)]
    pub placement: PlacementKind,
    /// Topic UUIDv7, required for --placement topic.
    #[arg(long)]
    pub topic_id: Option<String>,
    /// Root message UUIDv7, required for --placement thread.
    #[arg(long)]
    pub root_message_id: Option<String>,
    /// Message text. Conflicts with --text-file.
    #[arg(
        long,
        conflicts_with = "text_file",
        required_unless_present = "text_file"
    )]
    pub text: Option<String>,
    /// Read message text from this file. Conflicts with --text.
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with = "text",
        required_unless_present = "text"
    )]
    pub text_file: Option<PathBuf>,
    /// Reference a message UUIDv7. May be repeated, up to 64 total references.
    #[arg(long)]
    pub reference_message: Vec<String>,
    /// Reference a root thread UUIDv7. May be repeated, up to 64 total references.
    #[arg(long)]
    pub reference_thread: Vec<String>,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct MessageShowArguments {
    #[arg(long)]
    pub message_id: String,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum MessageScopeKind {
    Topic,
    Thread,
    Board,
    Project,
    AllProjects,
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum MessageSelectionKind {
    Latest,
    AfterPosition,
    Range,
}

#[derive(Args)]
pub(super) struct MessageListArguments {
    #[arg(long, value_enum)]
    pub scope: MessageScopeKind,
    #[arg(long)]
    pub topic_id: Option<String>,
    #[arg(long)]
    pub root_message_id: Option<String>,
    #[arg(long)]
    pub board_id: Option<String>,
    #[arg(long)]
    pub project_id: Option<String>,
    /// History selection; defaults to newest messages first.
    #[arg(long, value_enum, default_value = "latest")]
    pub selection: MessageSelectionKind,
    /// Exclusive lower activity bound for --selection after-position.
    #[arg(long)]
    pub after_activity_sequence: Option<u64>,
    /// Inclusive lower activity bound for --selection range.
    #[arg(long)]
    pub from_activity_sequence: Option<u64>,
    /// Inclusive upper activity bound for --selection range.
    #[arg(long)]
    pub to_activity_sequence: Option<u64>,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct ThreadShowArguments {
    #[arg(long)]
    pub root_message_id: String,
    /// Optional typed reader Identity JSON; when present, includes watch status.
    #[arg(long)]
    pub reader: Option<String>,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct ThreadMutationArguments {
    #[arg(long)]
    pub root_message_id: String,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct ThreadListArguments {
    #[arg(long)]
    pub project_id: String,
    /// Typed reader Identity JSON.
    #[arg(long)]
    pub reader: String,
    #[arg(long)]
    pub watched_only: bool,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct InboxFetchArguments {
    #[arg(long)]
    pub project_id: String,
    /// Typed reader Identity JSON. Fetching never acknowledges activity.
    #[arg(long)]
    pub reader: String,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum ReadScopeKind {
    Topic,
    Thread,
}

#[derive(Args)]
pub(super) struct InboxAcknowledgeArguments {
    #[arg(long, value_enum)]
    pub scope: ReadScopeKind,
    #[arg(long)]
    pub topic_id: Option<String>,
    #[arg(long)]
    pub root_message_id: Option<String>,
    #[arg(long)]
    pub through_activity_sequence: u64,
    #[command(flatten)]
    pub identity: MutationIdentityArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}

#[derive(Args)]
pub(super) struct InboxProjectsArguments {
    /// Typed reader Identity JSON.
    #[arg(long)]
    pub reader: String,
    #[arg(long)]
    pub unread_only: bool,
    #[command(flatten)]
    pub page: PageArguments,
    #[command(flatten)]
    pub common: CommonArguments,
}
