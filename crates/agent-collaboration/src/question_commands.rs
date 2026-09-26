//! Explicit question listing and single-use answers from the designated Approver.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

use clap::{Args, Parser, Subcommand};
use collaboration_client::protocol::{QuestionAnswerParams, QuestionAnswerValue, QuestionResponse};
use collaboration_client::{
    ClientError, ControlClient, OperationEffect, OperationFailureKind,
    operation_failure_from_client_error,
};

#[derive(Parser)]
#[command(name = "agent-collaboration question")]
struct QuestionArguments {
    #[command(subcommand)]
    command: QuestionCommand,
}

#[derive(Subcommand)]
enum QuestionCommand {
    List {
        #[arg(long)]
        pending: bool,
        #[command(flatten)]
        output: OutputArguments,
    },
    Answer {
        #[arg(long)]
        request_id: String,
        /// Exact typed Identity or SessionRef JSON for the designated Approver.
        #[arg(long)]
        actor: String,
        /// JSON object of field IDs to typed values for an answered form.
        #[arg(long, group = "answer")]
        content: Option<String>,
        #[arg(long, group = "answer")]
        decline: bool,
        #[arg(long, group = "answer")]
        cancel: bool,
        #[command(flatten)]
        output: OutputArguments,
    },
}

#[derive(Args)]
struct OutputArguments {
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

pub fn run_question_command(arguments: Vec<OsString>) -> i32 {
    let parsed = match crate::automation_argument_feedback::parse_arguments::<QuestionArguments>(
        arguments,
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (output, pending_only, answer) = match parsed.command {
        QuestionCommand::List { pending, output } => (output, pending, None),
        QuestionCommand::Answer {
            request_id,
            actor,
            content,
            decline,
            cancel,
            output,
        } => {
            let actor = match crate::approval_commands::parse_actor(&actor) {
                Ok(actor) => actor,
                Err(_) => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        "--actor must be typed Identity or exact SessionRef JSON",
                        2,
                        output.json,
                    );
                }
            };
            let response = match (content, decline, cancel) {
                (Some(content), false, false) => {
                    match serde_json::from_str::<BTreeMap<String, QuestionAnswerValue>>(&content) {
                        Ok(content) => QuestionResponse::Answered { content },
                        Err(_) => {
                            return crate::endpoint_commands::report_failure(
                                "invalidField",
                                "--content must be a JSON object of field answers",
                                2,
                                output.json,
                            );
                        }
                    }
                }
                (None, true, false) => QuestionResponse::Declined,
                (None, false, true) => QuestionResponse::Cancelled,
                _ => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        "Choose exactly one of --content, --decline, or --cancel",
                        2,
                        output.json,
                    );
                }
            };
            (
                output,
                false,
                Some(QuestionAnswerParams {
                    request_id,
                    actor,
                    response,
                }),
            )
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(output.service_directory) {
        Ok(directory) => directory,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &message,
                2,
                output.json,
            );
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return 3,
    };
    let result: Result<serde_json::Value, Box<collaboration_client::OperationFailure>> = runtime
        .block_on(async {
            let mut client = ControlClient::connect(
                &directory,
                "agent-collaboration",
                env!("CARGO_PKG_VERSION"),
            )
            .await
            .map_err(|error| {
                Box::new(operation_failure_from_client_error(
                    error,
                    OperationEffect::None,
                ))
            })?;
            let result = match answer {
                Some(params) => client
                    .answer_question(params)
                    .await
                    .map(serde_json::to_value)
                    .map_err(|error| Box::new(error.into_parts().0)),
                None => client
                    .list_questions(pending_only)
                    .await
                    .map(serde_json::to_value)
                    .map_err(|error| {
                        Box::new(operation_failure_from_client_error(
                            error,
                            OperationEffect::None,
                        ))
                    }),
            };
            let _ = client.close().await;
            result.and_then(|encoded| {
                encoded.map_err(|_| {
                    Box::new(operation_failure_from_client_error(
                        ClientError::Protocol("question output encoding failed"),
                        OperationEffect::None,
                    ))
                })
            })
        });
    match result {
        Ok(value) => {
            let rendered = if output.json {
                crate::endpoint_commands::result_envelope(value).to_string()
            } else {
                serde_json::to_string_pretty(&value).unwrap_or_default()
            };
            if writeln!(io::stdout(), "{rendered}").is_ok() {
                0
            } else {
                3
            }
        }
        Err(failure) => {
            let exit = match failure.kind {
                OperationFailureKind::Rejected => 4,
                _ if failure.effect == OperationEffect::Unknown => 5,
                OperationFailureKind::Timeout => 124,
                OperationFailureKind::UnsupportedCapability
                | OperationFailureKind::ProtocolViolation => 2,
                _ => 3,
            };
            if output.json {
                let _ = writeln!(
                    io::stdout(),
                    "{}",
                    serde_json::json!({"kind":"error","error":failure})
                );
            } else {
                let _ = writeln!(io::stderr(), "{}", failure.message);
            }
            exit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{QuestionArguments, QuestionCommand};
    use clap::Parser;

    #[test]
    fn answer_parser_rejects_conflicting_actions() {
        let base = ["question", "answer", "--request-id", "q-1", "--actor", "{}"];
        assert!(QuestionArguments::try_parse_from(base).is_ok());
        let parsed = QuestionArguments::try_parse_from([
            "question",
            "answer",
            "--request-id",
            "q-1",
            "--actor",
            "{}",
            "--decline",
        ])
        .expect("decline action");
        assert!(matches!(
            parsed.command,
            QuestionCommand::Answer { decline: true, .. }
        ));
        assert!(
            QuestionArguments::try_parse_from([
                "question",
                "answer",
                "--request-id",
                "q-1",
                "--actor",
                "{}",
                "--decline",
                "--cancel",
            ])
            .is_err()
        );
    }
}
