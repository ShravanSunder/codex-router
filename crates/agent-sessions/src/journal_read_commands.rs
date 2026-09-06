//! Explicit journal replay cursors; no inferred delivery, scheduling or model wake.
use crate::endpoint_commands::{report_failure, resolve_directory};
use clap::{Parser, Subcommand};
use communication_client::{ClientError, ControlClient};
use communication_protocol::{EndpointRef, JournalPosition};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    name = "journal",
    about = "Inspect or replay the rolling lifecycle journal without starting agent work"
)]
struct JournalArguments {
    #[arg(long, global = true)]
    service_directory: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: JournalCommand,
}
#[derive(Subcommand)]
enum JournalCommand {
    /// Report storage availability and retained sequence bounds.
    Status,
    /// Read after an explicit journal position. Continue with the returned next position.
    Read {
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        journal_id: String,
        #[arg(long, value_parser = clap::value_parser!(u64).range(0..=9007199254740991))]
        after: u64,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u64).range(0..=30000))]
        wait_milliseconds: u64,
    },
}
pub fn run_journal_command(arguments: Vec<OsString>) -> i32 {
    let args = match JournalArguments::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            return code;
        }
    };
    let directory = match resolve_directory(args.service_directory) {
        Ok(directory) => directory,
        Err(message) => return report_failure("invalidUsage", &message, 2, args.json),
    };
    let read = match args.command {
        JournalCommand::Status => None,
        JournalCommand::Read {
            endpoint,
            journal_id,
            after,
            page_size,
            wait_milliseconds,
        } => {
            let (Ok(endpoint), Ok(journal_id)) = (
                communication_protocol::EndpointId::try_from(endpoint),
                communication_protocol::UuidIdentity::try_from(journal_id),
            ) else {
                return report_failure(
                    "invalidUsage",
                    "Invalid endpoint or journal identity",
                    2,
                    args.json,
                );
            };
            Some((
                endpoint,
                JournalPosition {
                    journal_id,
                    sequence: after,
                },
                page_size,
                wait_milliseconds,
            ))
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return report_failure("unavailable", "Client runtime unavailable", 3, args.json),
    };
    let result = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-sessions", env!("CARGO_PKG_VERSION")).await?;
        let result = if let Some((endpoint_id, after, page_size, wait)) = read {
            let endpoint = EndpointRef {
                service_id: client.identity().service_id.clone(),
                endpoint_id,
            };
            json!(
                client
                    .read_journal(&endpoint, after, page_size, wait)
                    .await?
            )
        } else {
            json!(client.journal_status().await?)
        };
        let _closed = client.close().await;
        Ok::<_, ClientError>(result)
    });
    match result {
        Ok(result) => {
            let code = if result.get("storage").and_then(Value::as_str) == Some("unavailable") {
                3
            } else {
                0
            };
            let output = if args.json {
                json!({"kind":"result","result":result}).to_string()
            } else {
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "Result encoding failed".into())
            };
            if writeln!(io::stdout(), "{output}").is_err() {
                3
            } else {
                code
            }
        }
        Err(ClientError::Rejected { code, data }) => {
            let kind = data
                .as_ref()
                .and_then(|data| data.get("kind"))
                .and_then(Value::as_str)
                .unwrap_or("rejected");
            let exit = if code == -32602 {
                2
            } else if kind == "unavailable" {
                3
            } else {
                4
            };
            if args.json {
                let _printed = writeln!(
                    io::stdout(),
                    "{}",
                    json!({"kind":"error","error":{"kind":kind,"code":code,"data":data,"message":"Journal read rejected; cursor was not reset"}})
                );
                exit
            } else {
                report_failure(
                    kind,
                    "Journal read rejected; inspect bounds and choose an explicit cursor",
                    exit,
                    false,
                )
            }
        }
        Err(_) => report_failure(
            "unavailable",
            "Journal connection unavailable",
            3,
            args.json,
        ),
    }
}
