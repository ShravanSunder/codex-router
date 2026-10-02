use super::{
    HistoryArguments, InboxArguments, PushRecordShowArguments, operation_failure_exit,
    operation_failure_line, report_message_failure, validate_push_reference,
};
use collaboration_client::protocol::{
    PushRecordHistoryParams, PushRecordListParams, PushRecordListResult, PushRecordShowParams,
    PushRecordShowResult, SessionRef,
};
use collaboration_client::{
    ClientError, ControlClient, OperationEffect, operation_failure_from_client_error,
};
use serde_json::json;
use std::{
    io::{self, Write},
    path::PathBuf,
};

enum PushRecordQuery {
    Show(String),
    Inbox { limit: u32 },
    History { with: SessionRef, limit: u32 },
}

enum PushRecordReadResult {
    Show(Box<PushRecordShowResult>),
    List(PushRecordListResult),
}

pub(super) fn run_push_record_show(args: PushRecordShowArguments) -> i32 {
    if let Err(message) = validate_push_reference(&args.reference) {
        return report_message_failure("invalidField", &message, 2, args.json);
    }
    run_push_record_query(
        args.service_directory,
        args.json,
        PushRecordQuery::Show(args.reference),
    )
}

pub(super) fn run_message_inbox(args: InboxArguments) -> i32 {
    run_push_record_query(
        args.service_directory,
        args.json,
        PushRecordQuery::Inbox { limit: args.limit },
    )
}

pub(super) fn run_message_history(args: HistoryArguments) -> i32 {
    let with = match serde_json::from_str::<SessionRef>(&args.other_session) {
        Ok(value) => value,
        Err(_) => {
            return report_message_failure(
                "invalidField",
                "--with must be compact SessionRef JSON with endpoint.serviceId, endpoint.endpointId, and sessionId; for example --with '{\"endpoint\":{\"serviceId\":\"00000000-0000-4000-8000-000000000001\",\"endpointId\":\"codex-local\"},\"sessionId\":\"target-session\"}'",
                2,
                args.json,
            );
        }
    };
    run_push_record_query(
        args.service_directory,
        args.json,
        PushRecordQuery::History {
            with,
            limit: args.limit,
        },
    )
}

fn run_push_record_query(
    service_directory: Option<PathBuf>,
    machine: bool,
    query: PushRecordQuery,
) -> i32 {
    let harness_identity = match crate::current_session_identity::read_harness_session_identity() {
        Ok(identity) => identity,
        Err(error) => {
            return report_message_failure(
                "currentSessionUnavailable",
                &format!("{error}; run agent-collaboration whoami --json"),
                2,
                machine,
            );
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(service_directory) {
        Ok(directory) => directory,
        Err(message) => {
            return report_message_failure("invalidField", &message, 2, machine);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return report_message_failure("unavailable", "Client runtime unavailable", 3, machine);
        }
    };
    let outcome = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-collaboration", env!("CARGO_PKG_VERSION"))
                .await?;
        let caller = harness_identity
            .session_ref(&client.identity().service_id)
            .map_err(|_| ClientError::InvalidRequest("invalid current session identity"))?;
        let result = match query {
            PushRecordQuery::Show(reference) => client
                .router_show(PushRecordShowParams { caller, reference })
                .await
                .map(Box::new)
                .map(PushRecordReadResult::Show),
            PushRecordQuery::Inbox { limit } => client
                .message_inbox(PushRecordListParams { caller, limit })
                .await
                .map(PushRecordReadResult::List),
            PushRecordQuery::History { with, limit } => client
                .message_history(PushRecordHistoryParams {
                    caller,
                    with,
                    limit,
                })
                .await
                .map(PushRecordReadResult::List),
        };
        let _closed = client.close().await;
        result
    });
    report_push_record_read(outcome, machine)
}

fn report_push_record_read(
    result: Result<PushRecordReadResult, ClientError>,
    machine: bool,
) -> i32 {
    if let Err(error) = &result
        && let Some(code) = crate::permission_diagnostic_reporting::report_permission_error(
            error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            machine,
        )
    {
        return code;
    }
    match result {
        Ok(PushRecordReadResult::Show(show)) => {
            let written = if machine {
                writeln!(io::stdout(), "{}", push_record_show_envelope(*show))
            } else {
                writeln!(
                    io::stdout(),
                    "{}",
                    serde_json::to_string_pretty(&show)
                        .unwrap_or_else(|_| "Output unavailable".to_owned())
                )
            };
            if written.is_ok() { 0 } else { 3 }
        }
        Ok(PushRecordReadResult::List(list)) => {
            if machine {
                if writeln!(
                    io::stdout(),
                    "{}",
                    crate::endpoint_commands::result_envelope(json!(list))
                )
                .is_ok()
                {
                    0
                } else {
                    3
                }
            } else {
                let mut output = io::stdout().lock();
                for record in list.records {
                    if writeln!(output, "{}", record.line).is_err() {
                        return 3;
                    }
                }
                0
            }
        }
        Err(error) => {
            let failure = operation_failure_from_client_error(error, OperationEffect::None);
            let exit_code = operation_failure_exit(&failure);
            let human_failure = operation_failure_line(&failure, None);
            let record = json!({"kind":"error","error":failure});
            let written = if machine {
                writeln!(io::stdout(), "{record}")
            } else {
                writeln!(io::stderr(), "{human_failure}")
            };
            if written.is_ok() { exit_code } else { 5 }
        }
    }
}

fn push_record_show_envelope(show: PushRecordShowResult) -> serde_json::Value {
    let link = show.link;
    let activity_ranges = show.activity_ranges;
    let mut envelope = crate::endpoint_commands::result_envelope(json!(show.record));
    if let Some(result) = envelope
        .get_mut("result")
        .and_then(serde_json::Value::as_object_mut)
    {
        result.insert("link".to_owned(), json!(link));
        result.insert("activityRanges".to_owned(), json!(activity_ranges));
    }
    envelope
}
