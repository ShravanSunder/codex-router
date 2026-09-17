//! Unattended ACP conversation command using the reusable client and explicit cancellation.
use clap::{Args, Parser, Subcommand, ValueEnum};
use collaboration_client::protocol::ConversationRecord;
use collaboration_client::{AcpConversation, ClientError, ConversationEnd, ConversationEvent};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "agent-collaboration conversation",
    bin_name = "agent-collaboration conversation"
)]
struct ConversationArguments {
    #[command(subcommand)]
    command: ConversationCommand,
}
#[derive(Subcommand)]
enum ConversationCommand {
    /// Run an ACP prompt. Permission requests are cancelled, never automatically approved.
    Prompt(PromptArguments),
}
#[derive(Args)]
struct PromptArguments {
    #[arg(long)]
    endpoint: Option<String>,
    #[arg(long, conflicts_with_all = ["session"])]
    to: Option<String>,
    #[arg(long = "new", conflicts_with_all = ["session", "fork", "to"])]
    new_session: bool,
    #[arg(long, conflicts_with_all = ["new_session", "fork", "to"])]
    session: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["new_session", "session"])]
    fork: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    #[arg(long)]
    access: Option<ConversationAccess>,
    /// Exact SessionRef JSON for the client approval authority. Defaults to this session.
    #[arg(long)]
    approver: Option<String>,
    /// Board root message ID used to share owner-private scratch across sessions.
    #[arg(long)]
    root_message_id: Option<String>,
    #[arg(long)]
    cwd: PathBuf,
    #[arg(
        long,
        required_unless_present = "text_file",
        conflicts_with = "text_file"
    )]
    text: Option<String>,
    #[arg(long)]
    text_file: Option<PathBuf>,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long,default_value_t=300,value_parser=clap::value_parser!(u64).range(1..))]
    timeout_seconds: u64,
    #[arg(long)]
    json: bool,
}
#[derive(Clone, Copy, ValueEnum)]
enum ConversationAccess {
    WriteRestricted,
    WorkspaceWrite,
}
impl ConversationAccess {
    const fn as_str(self) -> &'static str {
        match self {
            Self::WriteRestricted => "write-restricted",
            Self::WorkspaceWrite => "workspace-write",
        }
    }
}
pub fn run_conversation_command(arguments: Vec<OsString>) -> i32 {
    let parsed = match crate::automation_argument_feedback::parse_arguments::<ConversationArguments>(
        arguments,
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let ConversationCommand::Prompt(args) = parsed.command;
    let prepared = prepare(&args);
    let (directory, text) = match prepared {
        Ok(v) => v,
        Err(e) => {
            return crate::endpoint_commands::report_failure("invalidField", &e, 2, args.json);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(v) => v,
        Err(_) => return 3,
    };
    runtime.block_on(async{
        let cancel=CancellationToken::new();let signal=cancel.clone();
        let signal_task=tokio::spawn(async move{let _signal=tokio::signal::ctrl_c().await;signal.cancel();});
        let mut target=None;
        let mut stage="connect";
        let result=async{
            let endpoint = if args.new_session {
                args.endpoint.clone().ok_or(ClientError::Protocol("--endpoint is required with --new"))?.try_into().map_err(|_| ClientError::Protocol("invalid endpoint ID"))?
            } else {
                conversation_target(&args).map_err(|_| ClientError::Protocol("invalid session target"))?.endpoint_id()
            };
            let mut client=AcpConversation::connect(&directory,endpoint).await?;
            let creator = if args.new_session || args.fork.is_some() {
                Some(current_session_ref(&client.endpoint().service_id).map_err(|_| ClientError::Protocol("current session identity unavailable"))?)
            } else { None };
            let approver = if let Some(value) = args.approver.as_deref() {
                Some(serde_json::from_str::<collaboration_client::protocol::SessionRef>(value).map_err(|_| ClientError::Protocol("invalid --approver SessionRef"))?)
            } else { creator.clone() };
            let selected = if args.new_session { None } else { Some(conversation_target(&args).map_err(|_| ClientError::Protocol("invalid session target"))?.resolve(&client.endpoint().service_id).map_err(|_| ClientError::Protocol("session target belongs to another service"))?) };
            let selected_id = selected.as_ref().map(|target| String::from(target.session_id.clone()));
            stage=if args.new_session{"new"}else if args.fork.is_some(){"fork"}else{"load"};
            let mut emit=|event|emit_record(event,args.json);
            target=Some(client.open_session(
                collaboration_client::ConversationSessionRequest {
                    session: selected_id.as_deref().filter(|_| args.fork.is_none()),
                    fork: selected_id.as_deref().filter(|_| args.fork.is_some()),
                    model: args.model.as_deref(),
                    effort: args.effort.as_deref(),
                    access: args.access.map(ConversationAccess::as_str),
                    created_by: creator.as_ref(),
                    approver: approver.as_ref(),
                    root_message_id: args.root_message_id.as_deref(),
                },
                &args.cwd,
                &mut emit,
            ).await?);
            stage="prompt";
            client.prompt(&text,args.effort.as_deref(),Duration::from_secs(args.timeout_seconds),cancel,&mut emit).await
        }.await;
        signal_task.abort();let _joined=signal_task.await;
        match result{
            Ok(ConversationEnd::Completed)=>0,
            Ok(ConversationEnd::TimedOut)=>124,
            Ok(ConversationEnd::Cancelled)=>130,
            Err(error)=>{
                if stage == "connect"
                    && let Some(code) = crate::permission_diagnostic_reporting::report_permission_error(
                        &error,
                        crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
                        args.json,
                    )
                {
                    return code;
                }
                let effect=if !matches!(error,ClientError::UnsupportedCapability(_)) && matches!(stage,"new"|"load"|"prompt"){"unknown"}else{"notDispatched"};
                let exit=match &error{ClientError::UnsupportedCapability(_)=>2,ClientError::Rejected{..}=>4,_ if effect=="unknown"=>5,_=>3};
                let message = match &error {
                    ClientError::Rejected { data: Some(data), .. } => {
                        format!("ACP conversation rejected: {data}; no operation replayed")
                    }
                    _ => "ACP conversation failed; no operation replayed".to_owned(),
                };
                let record=json!({"kind":"conversationError","target":target,"stage":stage,"effect":effect,"message":message});
                let _printed=writeln!(io::stdout(),"{record}");exit
            }
        }
    })
}
fn prepare(args: &PromptArguments) -> Result<(PathBuf, String), String> {
    let dispatch_count = usize::from(args.new_session)
        + usize::from(args.session.is_some())
        + usize::from(args.fork.as_ref().is_some_and(|value| !value.is_empty()))
        + usize::from(args.to.is_some());
    if dispatch_count != 1 {
        return Err("Choose exactly one of --new, --session, or --fork".into());
    }
    // Only a new thread must state its reasoning effort: resume keeps the
    // thread's persisted effort and fork inherits the source thread's.
    match args.effort.as_deref() {
        Some(effort) => validate_choice_value(effort, "--effort")?,
        None if args.new_session => return Err("--effort is required".into()),
        None => {}
    }
    if args.fork.is_none() && !args.new_session && args.model.is_some() {
        return Err(
            "--model is invalid with --session: model is fixed for a thread; fork to change it"
                .into(),
        );
    }
    if args.fork.is_none() && !args.new_session && args.access.is_some() {
        return Err("--access is invalid with --session: access is fixed for a thread".into());
    }
    if args.fork.is_none() && !args.new_session && args.approver.is_some() {
        return Err("--approver is valid only with --new or --fork".into());
    }
    if args.fork.is_none() && !args.new_session && args.root_message_id.is_some() {
        return Err(
            "--root-message-id is valid only with --new or --fork; resume retains its association"
                .into(),
        );
    }
    if args.new_session || args.fork.is_some() {
        // Fork inherits the source thread's model when the caller names none.
        match args.model.as_deref() {
            Some(model) => validate_choice_value(model, "--model")?,
            None if args.fork.is_some() => {}
            None => return Err("--model is required".into()),
        }
        args.access.as_ref().ok_or("--access is required")?;
        if let Some(root_message_id) = &args.root_message_id {
            let _: collaboration_client::protocol::UuidIdentity = root_message_id
                .clone()
                .try_into()
                .map_err(|_| "--root-message-id must be a canonical UUID")?;
        }
    }
    if !args.cwd.is_absolute() {
        return Err("ACP cwd must be absolute".into());
    }
    let directory = crate::endpoint_commands::resolve_directory(args.service_directory.clone())?;
    if args.new_session {
        let _: collaboration_client::protocol::EndpointId = args
            .endpoint
            .clone()
            .ok_or("--endpoint is required with --new")?
            .try_into()
            .map_err(|_| "Invalid endpoint ID")?;
    } else {
        let _ = conversation_target(args)?;
    }
    let text = if let Some(text) = &args.text {
        text.clone()
    } else {
        let path = args.text_file.as_ref().ok_or("Content required")?;
        let mut reader: Box<dyn Read> = if path.as_os_str() == "-" {
            Box::new(io::stdin())
        } else {
            Box::new(std::fs::File::open(path).map_err(|_| "Content file unavailable")?)
        };
        let mut text = String::new();
        reader
            .by_ref()
            .take((collaboration_client::protocol::MAX_CONTROL_FRAME_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|_| "Content must be UTF-8")?;
        text
    };
    let _: collaboration_client::protocol::MessageText = text
        .clone()
        .try_into()
        .map_err(|_| "Invalid or oversized content")?;
    Ok((directory, text))
}

fn current_session_ref(
    service_id: &collaboration_client::protocol::UuidIdentity,
) -> Result<collaboration_client::protocol::SessionRef, String> {
    let codex = std::env::var("CODEX_THREAD_ID")
        .ok()
        .filter(|value| !value.is_empty());
    let claude = std::env::var("CLAUDE_CODE_SESSION_ID")
        .ok()
        .filter(|value| !value.is_empty());
    let (endpoint_id, session_id) = match (codex, claude) {
        (Some(session_id), None) => ("codex-local", session_id),
        (None, Some(session_id)) => ("claude-local", session_id),
        _ => return Err("exactly one current session identity is required".into()),
    };
    Ok(collaboration_client::protocol::SessionRef {
        endpoint: collaboration_client::protocol::EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: endpoint_id
                .to_owned()
                .try_into()
                .map_err(|_| "invalid endpoint")?,
        },
        session_id: session_id.try_into().map_err(|_| "invalid session")?,
    })
}

fn conversation_target(
    args: &PromptArguments,
) -> Result<crate::session_target_arguments::ParsedSessionTarget, String> {
    let session = args.session.clone().or_else(|| {
        args.fork
            .as_ref()
            .filter(|value| !value.is_empty())
            .cloned()
    });
    crate::session_target_arguments::SessionTargetArguments {
        to: args.to.clone(),
        endpoint: args.endpoint.clone(),
        session,
    }
    .parse()
}

fn validate_choice_value(value: &str, flag: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().any(char::is_whitespace) {
        return Err(format!(
            "{flag} requires a non-empty value without whitespace"
        ));
    }
    Ok(())
}
fn emit_record(event: ConversationEvent, machine: bool) -> Result<(), ClientError> {
    let record = match event {
        ConversationEvent::SessionReady(target) => ConversationRecord::SessionReady { target },
        ConversationEvent::SessionUpdate { target, update } => {
            ConversationRecord::SessionUpdate { target, update }
        }
        ConversationEvent::PermissionRequired(target) => {
            ConversationRecord::PermissionRequired { target }
        }
        ConversationEvent::PromptResult { target, result } => {
            let effective_model = result
                .pointer("/_meta/codexRouter/effectiveModel")
                .and_then(serde_json::Value::as_str)
                .ok_or(ClientError::Protocol(
                    "effective model missing from prompt receipt",
                ))?
                .to_owned();
            let effective_effort = result
                .pointer("/_meta/codexRouter/effectiveEffort")
                .and_then(serde_json::Value::as_str)
                .ok_or(ClientError::Protocol(
                    "effective effort missing from prompt receipt",
                ))?
                .to_owned();
            let idle_seconds = result
                .pointer("/_meta/codexRouter/idleSeconds")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let effective_access = result
                .pointer("/_meta/codexRouter/effectiveAccess")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| ClientError::Protocol("invalid effective access in prompt receipt"))?;
            let settings_observation = serde_json::from_value(
                result
                    .pointer("/_meta/codexRouter/settingsObservation")
                    .cloned()
                    .ok_or(ClientError::Protocol(
                        "settings observation missing from prompt receipt",
                    ))?,
            )
            .map_err(|_| ClientError::Protocol("invalid settings observation in prompt receipt"))?;
            let effort_change: Option<collaboration_client::protocol::EffortChange> = result
                .pointer("/_meta/codexRouter/effortChange")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| ClientError::Protocol("invalid effort change in prompt receipt"))?;
            // A changed effort is allowed, and visible: the provider's prompt
            // cache for this session cannot be reused after it.
            if let (false, Some(change)) = (machine, effort_change.as_ref()) {
                let _noted = writeln!(
                    io::stderr(),
                    "note: effort changed from {} to {}; the provider prompt cache for this session will not be reused",
                    change.previous,
                    change.requested
                );
            }
            ConversationRecord::PromptResult {
                target,
                effective_model,
                effective_effort,
                effective_access,
                settings_observation: Box::new(settings_observation),
                effort_change,
                idle_seconds,
                result,
            }
        }
    };
    let record = serde_json::to_value(record)
        .map_err(|_| ClientError::Protocol("conversation output encoding failed"))?;
    if machine {
        writeln!(io::stdout(), "{record}")?;
    } else {
        writeln!(
            io::stdout(),
            "{}",
            serde_json::to_string_pretty(&record)
                .map_err(|_| ClientError::Protocol("conversation output encoding failed"))?
        )?;
    }
    Ok(())
}
