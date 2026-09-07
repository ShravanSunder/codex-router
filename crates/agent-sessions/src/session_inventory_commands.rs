//! Descriptive stored/loaded/active inventory through the public Control client.
use clap::{Parser, Subcommand, ValueEnum};
use communication_client::{ClientError, ControlClient};
use communication_protocol::{EndpointRef, NativeSessionListParams, NativeSessionView};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(name = "agent-sessions sessions", bin_name = "agent-sessions sessions")]
struct InventoryArguments {
    #[command(subcommand)]
    command: InventoryCommand,
}
#[derive(Clone, Copy, ValueEnum)]
enum InventoryView {
    Stored,
    Loaded,
    Active,
}
#[derive(Subcommand)]
enum InventoryCommand {
    /// Read stored metadata or currently loaded/active native observations; never resumes threads.
    List {
        #[arg(long)]
        endpoint: String,
        #[arg(long, value_enum)]
        view: InventoryView,
        #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}
pub fn run_session_inventory_command(arguments: Vec<OsString>) -> i32 {
    let parsed = match InventoryArguments::try_parse_from(arguments) {
        Ok(v) => v,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _printed = e.print();
            return code;
        }
    };
    let InventoryCommand::List {
        endpoint,
        view,
        page_size,
        cursor,
        service_directory,
        json: machine,
    } = parsed.command;
    let directory = match crate::endpoint_commands::resolve_directory(service_directory) {
        Ok(v) => v,
        Err(e) => return crate::endpoint_commands::report_failure("invalidUsage", &e, 2, machine),
    };
    let endpoint = match endpoint.try_into() {
        Ok(v) => v,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidUsage",
                "Invalid endpoint ID",
                2,
                machine,
            );
        }
    };
    if cursor
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > 1024)
    {
        return crate::endpoint_commands::report_failure(
            "invalidUsage",
            "Invalid inventory cursor",
            2,
            machine,
        );
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(v) => v,
        Err(_) => return 3,
    };
    let result = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "sessions-inventory", env!("CARGO_PKG_VERSION"))
                .await?;
        let params = NativeSessionListParams {
            endpoint: EndpointRef {
                service_id: client.identity().service_id.clone(),
                endpoint_id: endpoint,
            },
            view: match view {
                InventoryView::Stored => NativeSessionView::Stored,
                InventoryView::Loaded => NativeSessionView::Loaded,
                InventoryView::Active => NativeSessionView::Active,
            },
            page_size,
            cursor,
        };
        let result = client.list_sessions(params).await;
        let _closed = client.close().await;
        result
    });
    match result {
        Ok(result) => {
            let record = json!({"kind":"result","result":result});
            let text = if machine {
                record.to_string()
            } else {
                serde_json::to_string_pretty(&record).unwrap_or_default()
            };
            if writeln!(io::stdout(), "{text}").is_ok() {
                0
            } else {
                3
            }
        }
        Err(ClientError::Rejected { code, data }) => {
            let exit = if code == -32602 {
                2
            } else if data
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(serde_json::Value::as_str)
                == Some("unavailable")
            {
                3
            } else {
                4
            };
            if machine {
                let _printed = writeln!(
                    io::stdout(),
                    "{}",
                    json!({"kind":"error","error":{"code":code,"data":data}})
                );
            } else {
                let _printed = writeln!(io::stderr(), "Session inventory rejected");
            }
            exit
        }
        Err(_) => crate::endpoint_commands::report_failure(
            "unavailable",
            "Session inventory unavailable",
            3,
            machine,
        ),
    }
}
