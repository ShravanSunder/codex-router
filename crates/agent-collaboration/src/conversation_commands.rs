//! Unattended ACP conversation command using the reusable client and explicit cancellation.
use clap::{Args, Parser, Subcommand};
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
    endpoint: String,
    #[arg(long = "new", conflicts_with_all = ["session", "fork"])]
    new_session: bool,
    #[arg(long, conflicts_with_all = ["new_session", "fork"])]
    session: Option<String>,
    #[arg(long, conflicts_with_all = ["new_session", "session"])]
    fork: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
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
pub fn run_conversation_command(arguments: Vec<OsString>) -> i32 {
    let parsed = match crate::automation_argument_feedback::parse_arguments::<ConversationArguments>(
        arguments,
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let ConversationCommand::Prompt(args) = parsed.command;
    let prepared = prepare(&args);
    let (directory, endpoint, text) = match prepared {
        Ok(v) => v,
        Err(e) => {
            return crate::endpoint_commands::report_failure("invalidUsage", &e, 2, args.json);
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
            let mut client=AcpConversation::connect(&directory,endpoint).await?;
            stage=if args.new_session{"new"}else if args.fork.is_some(){"fork"}else{"load"};
            let mut emit=|event|emit_record(event,args.json);
            target=Some(client.open_session(
                collaboration_client::ConversationSessionRequest {
                    session: args.session.as_deref(),
                    fork: args.fork.as_deref(),
                    model: args.model.as_deref(),
                    effort: args.effort.as_deref().ok_or(ClientError::Protocol("--effort is required"))?,
                },
                &args.cwd,
                &mut emit,
            ).await?);
            stage="prompt";
            client.prompt(&text,args.effort.as_deref().ok_or(ClientError::Protocol("--effort is required"))?,Duration::from_secs(args.timeout_seconds),cancel,&mut emit).await
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
fn prepare(
    args: &PromptArguments,
) -> Result<(PathBuf, collaboration_client::protocol::EndpointId, String), String> {
    let dispatch_count = usize::from(args.new_session)
        + usize::from(args.session.is_some())
        + usize::from(args.fork.is_some());
    if dispatch_count != 1 {
        return Err("Choose exactly one of --new, --session, or --fork".into());
    }
    let effort = args.effort.as_deref().ok_or("--effort is required")?;
    validate_choice_value(effort, "--effort")?;
    if args.session.is_some() && args.model.is_some() {
        return Err(
            "--model is invalid with --session: model is fixed for a thread; fork to change it"
                .into(),
        );
    }
    if args.new_session || args.fork.is_some() {
        validate_choice_value(
            args.model.as_deref().ok_or("--model is required")?,
            "--model",
        )?;
    }
    if !args.cwd.is_absolute() {
        return Err("ACP cwd must be absolute".into());
    }
    let directory = crate::endpoint_commands::resolve_directory(args.service_directory.clone())?;
    let endpoint = args
        .endpoint
        .clone()
        .try_into()
        .map_err(|_| "Invalid endpoint ID")?;
    if let Some(session) = args.session.as_ref().or(args.fork.as_ref()) {
        let _: collaboration_client::protocol::SessionId = session
            .clone()
            .try_into()
            .map_err(|_| "Invalid session ID")?;
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
    Ok((directory, endpoint, text))
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
            ConversationRecord::PromptResult {
                target,
                effective_model,
                effective_effort,
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
