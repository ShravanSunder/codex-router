//! Scoped native event output; attachment is explicit and cancellation only closes observation.
use clap::{Parser, Subcommand};
use collaboration_client::NativeObservation;
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
    let EventCommand::Listen {
        endpoint,
        session,
        attach: _,
        service_directory,
        timeout_seconds,
    } = parsed.command;
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
    runtime.block_on(async {
        let attached=async {
            let endpoint_id=endpoint.try_into().map_err(|_|collaboration_client::ClientError::Protocol("invalid endpoint"))?;
            let session_id=session.try_into().map_err(|_|collaboration_client::ClientError::Protocol("invalid session"))?;
            NativeObservation::attach_by_ids(&directory,endpoint_id,session_id).await
        }.await;
        let mut observation=match attached {
            Ok(value)=>value,
            Err(error)=>return crate::permission_diagnostic_reporting::report_permission_error(
                &error,
                crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
                true,
            ).unwrap_or_else(||crate::endpoint_commands::report_failure("unavailable","Native attachment failed; no listener readiness",3,true)),
        };
        let target=observation.target().clone();let generation=observation.generation().clone();
        if writeln!(io::stdout(),"{}",json!({"kind":"listenerReady","target":target,"generation":generation})).is_err(){return 3;}
        let Some(deadline)=tokio::time::Instant::now().checked_add(Duration::from_secs(timeout_seconds)) else {return 2;};
        loop {
            let message=tokio::select! {
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
    })
}
