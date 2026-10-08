//! Prints the caller's own SessionRef so agents can pass it to MCP tools as `from`/`actor`.
use crate::current_session_identity::{HarnessSessionIdentity, read_harness_session_identity};
use crate::endpoint_commands::{report_failure, resolve_directory, result_envelope};
use clap::Parser;
use collaboration_client::{ClientError, CollaborationClient, protocol::SessionRef};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(name = "whoami")]
struct WhoamiArguments {
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

struct ResolvedCurrentSession {
    session: SessionRef,
    endpoint_registered: bool,
    machine_id: String,
    machine_label: String,
}

/// Resolves the harness session and binds it to the running Router service.
pub fn run_whoami_command(arguments: Vec<OsString>) -> i32 {
    let arguments =
        match crate::automation_argument_feedback::parse_arguments::<WhoamiArguments>(arguments) {
            Ok(value) => value,
            Err(code) => return code,
        };
    let machine_output = arguments.json;
    let harness = match read_harness_session_identity() {
        Ok(value) => value,
        Err(error) => {
            return report_failure("identityUnavailable", &error.to_string(), 2, machine_output);
        }
    };
    let directory = match resolve_directory(arguments.service_directory) {
        Ok(value) => value,
        Err(message) => return report_failure("invalidUsage", &message, 2, machine_output),
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return report_failure(
            "unavailable",
            "Client runtime unavailable",
            3,
            machine_output,
        );
    };
    match runtime.block_on(resolve_current_session(&directory, &harness)) {
        Ok(resolved) => print_current_session(&harness, &resolved, machine_output),
        Err(ResolveFailure::Invalid(message)) => {
            report_failure("identityUnavailable", &message, 2, machine_output)
        }
        Err(ResolveFailure::Client(error)) => {
            crate::permission_diagnostic_reporting::report_permission_error(
                &error,
                crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
                machine_output,
            )
            .unwrap_or_else(|| {
                report_failure(
                    "unavailable",
                    "Router service unavailable",
                    3,
                    machine_output,
                )
            })
        }
    }
}

enum ResolveFailure {
    Invalid(String),
    Client(ClientError),
}

async fn resolve_current_session(
    directory: &std::path::Path,
    harness: &HarnessSessionIdentity,
) -> Result<ResolvedCurrentSession, ResolveFailure> {
    let client =
        CollaborationClient::connect(directory, "agent-collaboration", env!("CARGO_PKG_VERSION"))
            .await
            .map_err(ResolveFailure::Client)?;
    let service_id = client.identity().service_id.clone();
    let machine_label = client.machine_label().as_str().to_owned();
    let inventory = client
        .list_endpoints()
        .await
        .map_err(ResolveFailure::Client)?;
    let session = harness
        .session_ref(&service_id)
        .map_err(ResolveFailure::Invalid)?;
    let endpoint_registered = inventory
        .endpoints
        .iter()
        .any(|entry| entry.endpoint == session.endpoint);
    Ok(ResolvedCurrentSession {
        session,
        endpoint_registered,
        machine_id: String::from(service_id),
        machine_label,
    })
}

fn print_current_session(
    harness: &HarnessSessionIdentity,
    resolved: &ResolvedCurrentSession,
    machine_output: bool,
) -> i32 {
    let mut out = io::stdout().lock();
    let printed = if machine_output {
        writeln!(
            out,
            "{}",
            result_envelope(json!({
                "session": resolved.session,
                "source": harness.harness.environment_variable,
                "endpointRegistered": resolved.endpoint_registered,
                "machineId": resolved.machine_id,
                "machineLabel": resolved.machine_label,
            }))
        )
    } else {
        let Ok(session) = serde_json::to_string(&resolved.session) else {
            return 3;
        };
        let registration = if resolved.endpoint_registered {
            String::new()
        } else {
            format!(
                "\nnote: endpoint {} is not registered with this Router; others cannot deliver to this session",
                harness.harness.endpoint_id
            )
        };
        let machine_label =
            collaboration_client::protocol::escape_push_line_field(&resolved.machine_label);
        writeln!(
            out,
            "machine: {} · {machine_label}\n{session}\nsource: {}{registration}",
            resolved.machine_id, harness.harness.environment_variable
        )
    };
    if printed.is_err() { 3 } else { 0 }
}
