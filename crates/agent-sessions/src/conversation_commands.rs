//! Unattended ACP conversation command using the reusable client and explicit cancellation.
use clap::{Args, Parser, Subcommand};
use communication_client::{AcpConversation, ClientError, ConversationEnd, ConversationEvent};
use communication_protocol::ConversationRecord;
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
    name = "agent-sessions conversation",
    bin_name = "agent-sessions conversation"
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
    #[arg(
        long = "new",
        required_unless_present = "session",
        conflicts_with = "session"
    )]
    new_session: bool,
    #[arg(long)]
    session: Option<String>,
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
    let parsed = match ConversationArguments::try_parse_from(arguments) {
        Ok(v) => v,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _printed = e.print();
            return code;
        }
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
            stage=if args.new_session{"new"}else{"load"};
            let mut emit=|event|emit_record(event,args.json);
            target=Some(client.open_session(args.session.as_deref(),&args.cwd,&mut emit).await?);
            stage="prompt";
            client.prompt(&text,Duration::from_secs(args.timeout_seconds),cancel,&mut emit).await
        }.await;
        signal_task.abort();let _joined=signal_task.await;
        match result{
            Ok(ConversationEnd::Completed)=>0,
            Ok(ConversationEnd::TimedOut)=>124,
            Ok(ConversationEnd::Cancelled)=>130,
            Err(error)=>{
                let effect=if !matches!(error,ClientError::UnsupportedCapability(_)) && matches!(stage,"new"|"load"|"prompt"){"unknown"}else{"notDispatched"};
                let exit=match &error{ClientError::UnsupportedCapability(_)=>2,ClientError::Rejected{..}=>4,_ if effect=="unknown"=>5,_=>3};
                let record=json!({"kind":"conversationError","target":target,"stage":stage,"effect":effect,"message":"ACP conversation failed; no operation replayed"});
                let _printed=writeln!(io::stdout(),"{record}");exit
            }
        }
    })
}
fn prepare(
    args: &PromptArguments,
) -> Result<(PathBuf, communication_protocol::EndpointId, String), String> {
    if !args.cwd.is_absolute() {
        return Err("ACP cwd must be absolute".into());
    }
    let directory = crate::endpoint_commands::resolve_directory(args.service_directory.clone())?;
    let endpoint = args
        .endpoint
        .clone()
        .try_into()
        .map_err(|_| "Invalid endpoint ID")?;
    if let Some(session) = &args.session {
        let _: communication_protocol::SessionId = session
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
            .take((communication_protocol::MAX_CONTROL_FRAME_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|_| "Content must be UTF-8")?;
        text
    };
    let _: communication_protocol::MessageText = text
        .clone()
        .try_into()
        .map_err(|_| "Invalid or oversized content")?;
    Ok((directory, endpoint, text))
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
            ConversationRecord::PromptResult { target, result }
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
