//! Descriptive stored/loaded/active inventory through the public Control client.
use clap::{Parser, Subcommand, ValueEnum};
use collaboration_client::protocol::{
    EndpointRef, NativeSessionListParams, NativeSessionScope, NativeSessionSource,
    NativeSessionView,
};
use collaboration_client::{ClientError, ControlClient};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    name = "agent-collaboration sessions",
    bin_name = "agent-collaboration sessions"
)]
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
#[derive(Clone, Copy, ValueEnum)]
enum InventorySource {
    Interactive,
    Subagents,
    All,
}
#[derive(Subcommand)]
enum InventoryCommand {
    /// Read stored metadata or currently loaded/active native observations; never resumes threads.
    List {
        #[arg(long)]
        endpoint: String,
        #[arg(long, value_enum)]
        view: InventoryView,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        checkout: Option<PathBuf>,
        #[arg(long)]
        repo: Option<PathBuf>,
        #[arg(long)]
        any: bool,
        #[arg(long, value_enum)]
        source: InventorySource,
        #[arg(long)]
        query: Option<String>,
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
    let parsed =
        match crate::automation_argument_feedback::parse_arguments::<InventoryArguments>(arguments)
        {
            Ok(v) => v,
            Err(code) => return code,
        };
    let InventoryCommand::List {
        endpoint,
        view,
        cwd,
        checkout,
        repo,
        any,
        source,
        query,
        page_size,
        cursor,
        service_directory,
        json: machine,
    } = parsed.command;
    let scope_count = usize::from(cwd.is_some())
        + usize::from(checkout.is_some())
        + usize::from(repo.is_some())
        + usize::from(any);
    if scope_count != 1 {
        return crate::endpoint_commands::report_failure(
            "invalidUsage",
            "Choose exactly one of --cwd, --checkout, --repo, or --any",
            2,
            machine,
        );
    }
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
            scope: if let Some(path) = cwd {
                NativeSessionScope::Cwd {
                    path: collaboration_client::session_catalog::normalize_path(&path),
                }
            } else if let Some(path) = checkout {
                NativeSessionScope::Checkout {
                    root: collaboration_client::session_catalog::checkout_root(&path),
                }
            } else if let Some(path) = repo {
                let identity =
                    collaboration_client::session_catalog::discover_repository_identity(&path);
                NativeSessionScope::Repo {
                    live_roots: identity.live_roots,
                    normalized_origin: identity.normalized_origin,
                    basename: identity.repository_basename,
                    fallback_cwd: identity.fallback_cwd,
                }
            } else {
                NativeSessionScope::Any
            },
            source: match source {
                InventorySource::Interactive => NativeSessionSource::Interactive,
                InventorySource::Subagents => NativeSessionSource::Subagents,
                InventorySource::All => NativeSessionSource::All,
            },
            query,
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
        Err(error) => crate::permission_diagnostic_reporting::report_permission_error(
            &error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            machine,
        )
        .unwrap_or_else(|| {
            crate::endpoint_commands::report_failure(
                "unavailable",
                "Session inventory unavailable",
                3,
                machine,
            )
        }),
    }
}
