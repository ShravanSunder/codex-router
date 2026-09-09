//! Descriptive endpoint discovery through the public Rust client.
use clap::{Parser, Subcommand};
use communication_client::{ClientError, ControlClient};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(name = "endpoints")]
struct EndpointArguments {
    #[command(subcommand)]
    command: EndpointCommand,
}
#[derive(Subcommand)]
enum EndpointCommand {
    List {
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}
/// Runs discovery without loading a thread or launching a backend.
pub fn run_endpoint_command(arguments: Vec<OsString>) -> i32 {
    let arguments = match EndpointArguments::try_parse_from(arguments) {
        Ok(value) => value,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            return code;
        }
    };
    let EndpointCommand::List {
        service_directory,
        json: machine_output,
    } = arguments.command;
    let directory = match resolve_directory(service_directory) {
        Ok(value) => value,
        Err(message) => return report_failure("invalidUsage", &message, 2, machine_output),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
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
        let inventory = client.list_endpoints().await?;
        client.close().await?;
        Ok::<_, ClientError>(inventory)
    });
    match result {
        Ok(inventory) => {
            let mut out = io::stdout().lock();
            if machine_output {
                if writeln!(out, "{}", json!({"kind":"result","result":inventory})).is_err() {
                    return 3;
                }
            } else {
                for entry in inventory.endpoints {
                    let id: String = entry.endpoint.endpoint_id.into();
                    let label: String = entry.label.into();
                    if writeln!(out, "{id}: {label} ({:?})", entry.availability).is_err() {
                        return 3;
                    }
                }
            }
            0
        }
        Err(ClientError::Rejected { .. }) => {
            report_failure("rejected", "Endpoint discovery rejected", 4, machine_output)
        }
        Err(_) => report_failure(
            "unavailable",
            "Endpoint discovery unavailable",
            3,
            machine_output,
        ),
    }
}
pub(crate) fn resolve_directory(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err("service directory must be absolute".into())
        };
    }
    let debug =
        cfg!(debug_assertions) && std::env::var_os("CODEX_ROUTER_USE_HOME_DEFAULT").is_none();
    if debug && let Some(root) = std::env::var_os("CODEX_ROUTER_DEBUG_ROUTER_ROOT") {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return Err("debug Router root must be absolute".into());
        }
        return Ok(root.join("agent-communication"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("absolute HOME required")?;
    Ok(home
        .join(if debug {
            ".codex-router-debug"
        } else {
            ".codex-router"
        })
        .join("agent-communication"))
}
pub(crate) fn report_failure(kind: &str, message: &str, code: i32, machine_output: bool) -> i32 {
    if machine_output {
        let error = if code == 2 && matches!(kind, "invalidField" | "invalidUsage") {
            let field = message
                .split_whitespace()
                .find(|word| word.starts_with("--"))
                .unwrap_or("arguments");
            json!({"kind":kind,"message":message,"stage":"validation",
                "field":field,"constraint":message,
                "effects":{"kind":"local","mutation":"none"},
                "nextAction":"correctRequest"})
        } else {
            json!({"kind":kind,"message":message})
        };
        let _printed = writeln!(io::stdout(), "{}", json!({"kind":"error","error":error}));
    } else {
        let _printed = writeln!(io::stderr(), "{message}");
    }
    code
}
