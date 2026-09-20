//! Scoped native event output; attachment is explicit and cancellation only closes observation.
use clap::{Parser, Subcommand};
use collaboration_client::protocol::{EndpointId, EndpointRef, SessionId, SessionRef};
use collaboration_client::{BoundedObservationRequest, ControlClient, NativeObservation};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "agent-collaboration events",
    bin_name = "agent-collaboration events"
)]
struct EventArguments {
    #[command(subcommand)]
    command: EventCommand,
}
#[derive(Subcommand)]
enum EventCommand {
    /// Attach/load a native thread and stream its events. Closing does not interrupt work.
    Listen {
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        session: String,
        #[arg(long, required = true)]
        attach: bool,
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long,default_value_t=60,value_parser=clap::value_parser!(u64).range(1..))]
        timeout_seconds: u64,
    },
    /// Attach/load a native thread and return one bounded JSON observation result.
    Observe {
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        session: String,
        #[arg(long, required = true)]
        attach: bool,
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long,default_value_t=60,value_parser=clap::value_parser!(u64).range(1..))]
        timeout_seconds: u64,
        #[arg(long, default_value_t = 256, value_parser = parse_max_events)]
        max_events: usize,
        #[arg(long, default_value_t = 262_144, value_parser = parse_max_bytes)]
        max_bytes: usize,
    },
}

fn parse_max_events(value: &str) -> Result<usize, String> {
    parse_bounded_usize(value, 4096, "--max-events")
}

fn parse_max_bytes(value: &str) -> Result<usize, String> {
    parse_bounded_usize(value, 1_048_576, "--max-bytes")
}

fn parse_bounded_usize(value: &str, maximum: usize, flag: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{flag} requires a positive integer"))?;
    if parsed == 0 || parsed > maximum {
        return Err(format!("{flag} must be between 1 and {maximum}"));
    }
    Ok(parsed)
}
pub fn run_event_command(arguments: Vec<OsString>) -> i32 {
    let parsed = match EventArguments::try_parse_from(arguments) {
        Ok(value) => value,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            return code;
        }
    };
    let service_directory = match &parsed.command {
        EventCommand::Listen {
            service_directory, ..
        }
        | EventCommand::Observe {
            service_directory, ..
        } => service_directory.clone(),
    };
    let directory = match crate::endpoint_commands::resolve_directory(service_directory) {
        Ok(value) => value,
        Err(error) => {
            return crate::endpoint_commands::report_failure("invalidUsage", &error, 2, true);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => return 3,
    };
    runtime.block_on(async move {
        match parsed.command {
            EventCommand::Listen {
                endpoint,
                session,
                attach: _,
                service_directory: _,
                timeout_seconds,
            } => listen(&directory, endpoint, session, timeout_seconds).await,
            EventCommand::Observe {
                endpoint,
                session,
                attach: _,
                service_directory: _,
                timeout_seconds,
                max_events,
                max_bytes,
            } => {
                observe(
                    &directory,
                    endpoint,
                    session,
                    timeout_seconds,
                    max_events,
                    max_bytes,
                )
                .await
            }
        }
    })
}

async fn listen(
    directory: &std::path::Path,
    endpoint: String,
    session: String,
    timeout_seconds: u64,
) -> i32 {
    let attached = async {
        let endpoint_id = endpoint.try_into().map_err(|_| {
            collaboration_client::OperationError::before_dispatch(
                "observation-validation",
                None,
                collaboration_client::ClientError::Protocol("invalid endpoint"),
            )
        })?;
        let session_id = session.try_into().map_err(|_| {
            collaboration_client::OperationError::before_dispatch(
                "observation-validation",
                None,
                collaboration_client::ClientError::Protocol("invalid session"),
            )
        })?;
        NativeObservation::attach_by_ids_with_context(directory, endpoint_id, session_id).await
    }
    .await;
    let mut observation = match attached {
        Ok(value) => value,
        Err(error) => {
            if let Some(exit) = crate::permission_diagnostic_reporting::report_permission_error(
                error.source(),
                crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
                true,
            ) {
                return exit;
            }
            return report_observation_error(error);
        }
    };
    let target = observation.target().clone();
    let generation = observation.generation().clone();
    if writeln!(
        io::stdout(),
        "{}",
        json!({"kind":"listenerReady","target":target,"generation":generation})
    )
    .is_err()
    {
        return 3;
    }
    let Some(deadline) =
        tokio::time::Instant::now().checked_add(Duration::from_secs(timeout_seconds))
    else {
        return 2;
    };
    loop {
        let message = tokio::select! {
            _=tokio::time::sleep_until(deadline)=>{
                let _printed=writeln!(io::stdout(),"{}",json!({"kind":"connectionClosed","reason":"observationTimeout"}));return 124;
            },
            _=tokio::signal::ctrl_c()=>{
                let _printed=writeln!(io::stdout(),"{}",json!({"kind":"connectionClosed","reason":"callerCancelled"}));return 130;
            },
            message=observation.next_message()=>message,
        };
        match message {
                Ok(message)=>if writeln!(io::stdout(),"{}",json!({"kind":"nativeMessage","target":target,"generation":generation,"message":message})).is_err(){return 3;},
                Err(_)=>{let _printed=writeln!(io::stdout(),"{}",json!({"kind":"connectionClosed","reason":"nativeConnectionLost"}));return 3;}
            }
    }
}

fn report_observation_error(error: collaboration_client::OperationError) -> i32 {
    let (failure, target, turn_id) = error.into_parts();
    let exit = if failure.effect == collaboration_client::OperationEffect::Unknown {
        5
    } else if failure.kind == collaboration_client::OperationFailureKind::Timeout {
        124
    } else {
        3
    };
    let record = serde_json::json!({
        "kind":"error", "target":target, "turnId":turn_id, "error":failure
    });
    let _printed = writeln!(io::stdout(), "{record}");
    exit
}

async fn observe(
    directory: &std::path::Path,
    endpoint: String,
    session: String,
    timeout_seconds: u64,
    max_events: usize,
    max_bytes: usize,
) -> i32 {
    let endpoint_id = match EndpointId::try_from(endpoint) {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidUsage",
                "invalid endpoint",
                2,
                true,
            );
        }
    };
    let session_id = match SessionId::try_from(session) {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidUsage",
                "invalid session",
                2,
                true,
            );
        }
    };
    let control =
        match ControlClient::connect(directory, "agent-collaboration", env!("CARGO_PKG_VERSION"))
            .await
        {
            Ok(value) => value,
            Err(error) => {
                return crate::permission_diagnostic_reporting::report_permission_error(
                    &error,
                    crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
                    true,
                )
                .unwrap_or_else(|| {
                    crate::endpoint_commands::report_failure(
                        "unavailable",
                        "Control discovery failed",
                        3,
                        true,
                    )
                });
            }
        };
    let target = SessionRef {
        endpoint: EndpointRef {
            service_id: control.identity().service_id.clone(),
            endpoint_id,
        },
        session_id,
    };
    drop(control);
    let cancel = tokio_util::sync::CancellationToken::new();
    let observation = NativeObservation::observe_bounded(
        directory,
        BoundedObservationRequest {
            target,
            timeout_seconds,
            max_events,
            max_bytes,
        },
        cancel.clone(),
    );
    tokio::pin!(observation);
    let result = tokio::select! {
        result = &mut observation => result,
        _ = tokio::signal::ctrl_c() => {
            cancel.cancel();
            observation.await
        }
    };
    match result {
        Ok(result) => match serde_json::to_writer(io::stdout(), &result)
            .and_then(|()| writeln!(io::stdout()).map_err(serde_json::Error::io))
        {
            Ok(()) => 0,
            Err(_) => 3,
        },
        Err(error) => {
            let (failure, target, turn_id) = error.into_parts();
            let exit = if failure.effect == collaboration_client::OperationEffect::Unknown {
                5
            } else if failure.kind == collaboration_client::OperationFailureKind::Timeout {
                124
            } else {
                3
            };
            let record = serde_json::json!({
                "kind":"error", "target":target, "turnId":turn_id, "error":failure
            });
            let _printed = writeln!(io::stdout(), "{record}");
            exit
        }
    }
}
