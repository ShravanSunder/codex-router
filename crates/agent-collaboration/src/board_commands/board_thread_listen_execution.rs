//! Process-owned Thread Listen delivery writes each Batch set directly to stdout.
use super::board_preparation::{self, CommandContext, PendingThreadListen};
use collaboration_client::board::*;
use collaboration_client::{BoardClientError, ControlClient};
use serde_json::json;
use std::io::{self, Write};
use std::time::Duration;

pub(super) fn execute(pending: PendingThreadListen, context: CommandContext) -> i32 {
    let directory = match crate::endpoint_commands::resolve_directory(context.service_directory) {
        Ok(directory) => directory,
        Err(message) => return report_error("invalidUsage", &message),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return report_error("boardUnavailable", "Thread Listen runtime unavailable"),
    };
    runtime.block_on(run(pending, directory))
}

pub(super) fn execute_show(request: ThreadListenShowRequest, context: CommandContext) -> i32 {
    execute_control(context, |client| {
        Box::pin(async move {
            client
                .board_thread_listen_show(request)
                .await
                .and_then(render_control_result)
        })
    })
}

pub(super) fn execute_cancel(request: ThreadListenCancelRequest, context: CommandContext) -> i32 {
    execute_control(context, |client| {
        Box::pin(async move {
            client
                .board_thread_listen_cancel(request)
                .await
                .and_then(render_control_result)
        })
    })
}

fn execute_control<TAction>(context: CommandContext, action: TAction) -> i32
where
    TAction: for<'client> FnOnce(
        &'client mut ControlClient,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<serde_json::Value, BoardClientError>> + 'client,
        >,
    >,
{
    let directory = match crate::endpoint_commands::resolve_directory(context.service_directory) {
        Ok(directory) => directory,
        Err(message) => return report_error("invalidUsage", &message),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return report_error("boardUnavailable", "Thread Listen runtime unavailable"),
    };
    runtime.block_on(async {
        let mut client = match ControlClient::connect(
            &directory,
            "agent-collaboration-thread-listen",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        {
            Ok(client) => client,
            Err(error) => return report_client_error(&error.to_string()),
        };
        let result = action(&mut client).await;
        let _closed = client.close().await;
        match result {
            Ok(result) => {
                let value = json!({"kind":"result","result":result});
                if writeln!(io::stdout().lock(), "{value}").is_ok() {
                    0
                } else {
                    1
                }
            }
            Err(error) => report_board_error(&error),
        }
    })
}

fn render_control_result<TValue: serde::Serialize>(
    result: TValue,
) -> Result<serde_json::Value, BoardClientError> {
    serde_json::to_value(result).map_err(|_| {
        collaboration_client::ClientError::Protocol("Thread Listen result could not be rendered")
            .into()
    })
}

async fn run(mut pending: PendingThreadListen, directory: std::path::PathBuf) -> i32 {
    let mut client = match ControlClient::connect(
        &directory,
        "agent-collaboration-thread-listen",
        env!("CARGO_PKG_VERSION"),
    )
    .await
    {
        Ok(client) => client,
        Err(error) => return report_client_error(&error.to_string()),
    };
    let request = match board_preparation::finalize_actor(&pending.actor, &client) {
        Ok(reader) => {
            pending.request.reader = reader;
            pending.request
        }
        Err(message) => {
            return crate::endpoint_commands::report_failure("invalidField", &message, 2, true);
        }
    };
    let code = run_with_client(request, &mut client).await;
    let _closed = client.close().await;
    code
}

pub(super) async fn run_with_client(
    request: ThreadListenRequest,
    client: &mut ControlClient,
) -> i32 {
    let lifetime_seconds = match request.mode {
        ThreadListenMode::Once { max_wait_seconds } => max_wait_seconds,
        ThreadListenMode::Repeating { lifetime_seconds } => lifetime_seconds,
    };
    let request_timeout = Duration::from_secs(lifetime_seconds)
        .checked_add(Duration::from_secs(5))
        .unwrap_or(Duration::MAX);
    let acknowledge = request.acknowledge;
    let reader = request.reader.clone();
    let listen = match client.board_thread_listen(request).await {
        Ok(result) => result.listen,
        Err(error) => return report_board_error(&error),
    };
    let mut emitted = false;
    loop {
        let result = match client
            .board_thread_wait(
                ThreadWaitRequest {
                    listen_id: listen.listen_id.clone(),
                },
                request_timeout,
            )
            .await
        {
            Ok(result) => result,
            Err(error) => return report_board_error(&error),
        };
        if let Some(batch_set) = result.batch_set {
            if write_batch_set(&batch_set).is_err() {
                return 1;
            }
            emitted = true;
            if acknowledge {
                for batch in &batch_set.batches {
                    if let Err(error) = client
                        .board_inbox_acknowledge(InboxAcknowledgeRequest {
                            actor: reader.clone(),
                            acting_for: None,
                            scope: ReadScope::Thread {
                                root_message_id: batch.root_message_id.clone(),
                            },
                            through_activity_sequence: batch.delivered_through,
                        })
                        .await
                    {
                        return report_board_error(&error);
                    }
                }
            }
        }
        if let Some(end) = result.end {
            return thread_listen_exit_code(end.reason, emitted);
        }
    }
}

fn thread_listen_exit_code(reason: ThreadListenEndReason, emitted: bool) -> i32 {
    match reason {
        ThreadListenEndReason::Emitted | ThreadListenEndReason::Cancelled => 0,
        ThreadListenEndReason::Timeout | ThreadListenEndReason::Lifetime => {
            if emitted {
                0
            } else {
                3
            }
        }
        ThreadListenEndReason::Error => 1,
    }
}

fn write_batch_set(batch_set: &ThreadListenBatchSet) -> io::Result<()> {
    let serialized = serde_json::to_string(batch_set).map_err(io::Error::other)?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{serialized}")?;
    stdout.flush()
}

fn report_board_error(error: &BoardClientError) -> i32 {
    match error {
        BoardClientError::Rejected(error) => {
            let value = super::board_execution::refusal_output(error, None);
            let _written = writeln!(io::stdout().lock(), "{value}");
            1
        }
        _ => report_client_error(&error.to_string()),
    }
}

fn report_client_error(message: &str) -> i32 {
    report_error("boardUnavailable", message)
}

fn report_error(kind: &str, message: &str) -> i32 {
    let value = json!({"kind":"error","error":{"kind":kind,"message":message}});
    let _written = writeln!(io::stdout().lock(), "{value}");
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeating_lifetime_exit_depends_on_whether_a_batch_was_emitted() {
        assert_eq!(
            thread_listen_exit_code(ThreadListenEndReason::Lifetime, false),
            3
        );
        assert_eq!(
            thread_listen_exit_code(ThreadListenEndReason::Lifetime, true),
            0
        );
    }

    #[test]
    fn once_timeout_and_errors_keep_their_exact_exit_codes() {
        assert_eq!(
            thread_listen_exit_code(ThreadListenEndReason::Timeout, false),
            3
        );
        assert_eq!(
            thread_listen_exit_code(ThreadListenEndReason::Error, false),
            1
        );
    }
}
