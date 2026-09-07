//! Read-only historical address discovery through the public SDK.
use crate::endpoint_commands::{report_failure, resolve_directory};
use clap::{Parser, Subcommand};
use communication_client::{ClientError, ControlClient};
use communication_protocol::{EndpointId, EndpointRef};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    name = "addresses",
    about = "Inspect remembered thread addresses and observation coverage; never load or wake agents"
)]
struct AddressArguments {
    #[command(subcommand)]
    command: AddressCommand,
}
#[derive(Subcommand)]
enum AddressCommand {
    /// Read one immutable snapshot page; use nextCursor to continue the same snapshot.
    List {
        #[arg(long)]
        endpoint: String,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}
pub fn run_address_command(arguments: Vec<OsString>) -> i32 {
    let arguments = match AddressArguments::try_parse_from(arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            let status = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            return status;
        }
    };
    let AddressCommand::List {
        endpoint,
        page_size,
        cursor,
        service_directory,
        json: machine_output,
    } = arguments.command;
    let directory = match resolve_directory(service_directory) {
        Ok(directory) => directory,
        Err(message) => return report_failure("invalidUsage", &message, 2, machine_output),
    };
    let endpoint = match EndpointId::try_from(endpoint) {
        Ok(endpoint) => endpoint,
        Err(_) => {
            return report_failure(
                "invalidUsage",
                "Invalid endpoint identifier",
                2,
                machine_output,
            );
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return report_failure(
                "unavailable",
                "Client runtime unavailable",
                3,
                machine_output,
            );
        }
    };
    let result = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-sessions", env!("CARGO_PKG_VERSION")).await?;
        let target = EndpointRef {
            service_id: client.identity().service_id.clone(),
            endpoint_id: endpoint,
        };
        let snapshot = client
            .list_addresses(&target, page_size, cursor.as_deref())
            .await?;
        client.close().await?;
        Ok::<_, ClientError>(snapshot)
    });
    match result {
        Ok(snapshot) => {
            let mut output = io::stdout().lock();
            let written = if machine_output {
                writeln!(
                    output,
                    "{}",
                    serde_json::json!({"kind":"result","result":snapshot})
                )
            } else {
                (|| {
                    writeln!(
                        output,
                        "Observed coverage: {:?}; {} remembered addresses",
                        snapshot.coverage.state,
                        snapshot.entries.len()
                    )?;
                    for entry in snapshot.entries {
                        writeln!(
                            output,
                            "{}: {:?}; last observed status {:?}",
                            String::from(entry.address.native_thread_id),
                            entry.disposition.existence,
                            entry.last_status
                        )?;
                    }
                    if let Some(cursor) = snapshot.next_cursor {
                        writeln!(output, "Continue this snapshot with --cursor {cursor}")?;
                    }
                    Ok::<_, io::Error>(())
                })()
            };
            if written.is_ok() { 0 } else { 3 }
        }
        Err(ClientError::Rejected { .. }) => {
            report_failure("rejected", "Address snapshot rejected", 4, machine_output)
        }
        Err(_) => report_failure(
            "unavailable",
            "Address snapshot unavailable",
            3,
            machine_output,
        ),
    }
}
