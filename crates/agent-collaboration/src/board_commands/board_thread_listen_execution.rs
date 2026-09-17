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
    execute_control(context, None, |client| {
        Box::pin(async move {
            client
                .board_thread_listen_show(request)
                .await
                .and_then(render_control_result)
        })
    })
}

pub(super) fn execute_cancel(request: ThreadListenCancelRequest, context: CommandContext) -> i32 {
    execute_control(context, Some(json!({"reason":"cancelled"})), |client| {
        Box::pin(async move {
            client
                .board_thread_listen_cancel(request)
                .await
                .and_then(render_control_result)
        })
    })
}

/// Publishes one Control result. A command that changes state names its effects;
/// a read passes `None`.
fn execute_control<TAction>(
    context: CommandContext,
    effects: Option<serde_json::Value>,
    action: TAction,
) -> i32
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
                let value = match effects {
                    Some(effects) => {
                        crate::endpoint_commands::mutation_envelope(json!(result), effects)
                    }
                    None => crate::endpoint_commands::result_envelope(json!(result)),
                };
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
            if pending.request.delivery == ThreadListenDelivery::Session
                && !board_preparation::is_codex_session_identity(&reader)
            {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "--deliver session requires the calling codex-local session identity",
                    2,
                    true,
                );
            }
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
    let should_acknowledge = request.acknowledge;
    let reader = request.reader.clone();
    // The first delivered Batch set decides catchUp; --from is not a proxy for it.
    let mut catch_up = false;
    let delivery = request.delivery;
    let listen = match client.board_thread_listen(request).await {
        Ok(result) => result.listen,
        Err(error) => return report_board_error(&error),
    };
    if delivery == ThreadListenDelivery::Session {
        return write_session_armed(&listen);
    }
    let mut emitted = false;
    let mut batches_delivered = 0_u64;
    let mut first_sequence = None;
    let mut last_sequence = None;
    let mut acknowledged = false;
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
            Err(error) => {
                let finalization = ThreadListenFinalization {
                    kind: ThreadListenFinalizationKind::ListenEnd,
                    listen_id: listen.listen_id.clone(),
                    reason: ThreadListenEndReason::Error,
                    batches_delivered,
                    first_sequence,
                    last_sequence,
                    catch_up,
                    acknowledged,
                    last_rejection: None,
                };
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "{}",
                    serde_json::json!(finalization)
                );
                return report_board_error(&error);
            }
        };
        if let Some(batch_set) = result.batch_set {
            if !emitted {
                catch_up = batch_set.catch_up;
            }
            if write_batch_set(&batch_set).is_err() {
                return 1;
            }
            emitted = true;
            batches_delivered += 1;
            let batch_first = batch_set
                .batches
                .iter()
                .flat_map(|batch| batch.messages.iter())
                .map(|message| message.activity_sequence)
                .min();
            first_sequence = first_sequence.or(batch_first);
            last_sequence = batch_set
                .batches
                .iter()
                .map(|batch| batch.delivered_through)
                .max()
                .or(last_sequence);
            if should_acknowledge {
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
                        let finalization = ThreadListenFinalization {
                            kind: ThreadListenFinalizationKind::ListenEnd,
                            listen_id: listen.listen_id.clone(),
                            reason: ThreadListenEndReason::Error,
                            batches_delivered,
                            first_sequence,
                            last_sequence,
                            catch_up,
                            acknowledged: false,
                            last_rejection: None,
                        };
                        let _ = writeln!(
                            std::io::stdout().lock(),
                            "{}",
                            serde_json::json!(finalization)
                        );
                        return report_board_error(&error);
                    }
                }
                acknowledged = true;
            }
        }
        if let Some(end) = result.end {
            let finalization = ThreadListenFinalization {
                kind: ThreadListenFinalizationKind::ListenEnd,
                listen_id: end.listen_id.clone(),
                reason: end.reason,
                batches_delivered,
                first_sequence,
                last_sequence,
                catch_up,
                acknowledged,
                last_rejection: None,
            };
            if writeln!(
                std::io::stdout().lock(),
                "{}",
                serde_json::json!(finalization)
            )
            .is_err()
            {
                return 1;
            }
            return thread_listen_exit_code(end.reason, emitted);
        }
    }
}

fn write_session_armed(listen: &ThreadListenSnapshot) -> i32 {
    let value = crate::endpoint_commands::result_envelope(serde_json::json!(listen));
    if writeln!(std::io::stdout().lock(), "{value}").is_ok() {
        0
    } else {
        1
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
