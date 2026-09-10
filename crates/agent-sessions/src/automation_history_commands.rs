//! CLI entrypoints for delivery evidence, instruction revisions and durable operation receipts.
use crate::automation_collection_commands::{
    CollectionCommand, CollectionContext, DeliveryListOptions, RevisionListOptions,
    run_collection_command,
};
use clap::{Parser, Subcommand};
use std::{ffi::OsString, path::PathBuf};

#[derive(Parser)]
#[command(name = "agent-sessions delivery", bin_name = "agent-sessions delivery")]
struct DeliveryArguments {
    #[arg(long, global = true)]
    service_directory: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: DeliveryCommand,
}
#[derive(Subcommand)]
enum DeliveryCommand {
    /// Recover positive native acceptance evidence without resending input; absence stays uncertain.
    Reconcile {
        #[arg(long)]
        delivery_id: String,
    },
    /// Inspect attempts without replaying input; older evidence may have expired.
    Attempts(crate::automation_collection_commands::DeliveryAttemptOptions),
    /// List durable obligations, optionally restricted to an exact wake-up.
    List(DeliveryListOptions),
    /// Inspect pending, accepted or uncertain delivery evidence without sending it again.
    Show {
        #[arg(long)]
        delivery_id: String,
    },
}
pub fn run_delivery_command(arguments: Vec<OsString>) -> i32 {
    let args: DeliveryArguments =
        match crate::automation_argument_feedback::parse_arguments(arguments) {
            Ok(args) => args,
            Err(code) => return code,
        };
    let command = match args.command {
        DeliveryCommand::Reconcile { delivery_id } => {
            CollectionCommand::DeliveryReconcile(delivery_id)
        }
        DeliveryCommand::Attempts(options) => CollectionCommand::DeliveryAttempts(options),
        DeliveryCommand::List(options) => CollectionCommand::Deliveries(options),
        DeliveryCommand::Show { delivery_id } => CollectionCommand::DeliveryShow(delivery_id),
    };
    run_collection_command(
        command,
        CollectionContext {
            service_directory: args.service_directory,
            json: args.json,
        },
    )
}
#[derive(Parser)]
#[command(name = "agent-sessions revision", bin_name = "agent-sessions revision")]
struct RevisionArguments {
    #[arg(long, global = true)]
    service_directory: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: RevisionCommand,
}
#[derive(Subcommand)]
enum RevisionCommand {
    /// List immutable instruction snapshots; these do not expire with automation events.
    List(RevisionListOptions),
}
pub fn run_revision_command(arguments: Vec<OsString>) -> i32 {
    let args: RevisionArguments =
        match crate::automation_argument_feedback::parse_arguments(arguments) {
            Ok(args) => args,
            Err(code) => return code,
        };
    let RevisionCommand::List(options) = args.command;
    run_collection_command(
        CollectionCommand::Revisions(options),
        CollectionContext {
            service_directory: args.service_directory,
            json: args.json,
        },
    )
}

#[derive(Parser)]
#[command(
    name = "agent-sessions operation",
    bin_name = "agent-sessions operation"
)]
struct OperationArguments {
    #[arg(long, global = true)]
    service_directory: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: OperationCommand,
}
#[derive(Subcommand)]
enum OperationCommand {
    /// Recover the original command receipt without resending native work. Unresolved effects remain uncertain.
    Reconcile {
        #[arg(long)]
        operation_id: String,
    },
    /// Inspect an original durable command receipt; success does not imply its worker task completed.
    Show {
        #[arg(long)]
        operation_id: String,
    },
}
pub fn run_operation_command(arguments: Vec<OsString>) -> i32 {
    let args: OperationArguments =
        match crate::automation_argument_feedback::parse_arguments(arguments) {
            Ok(args) => args,
            Err(code) => return code,
        };
    let command = match args.command {
        OperationCommand::Show { operation_id } => CollectionCommand::OperationShow(operation_id),
        OperationCommand::Reconcile { operation_id } => {
            CollectionCommand::OperationReconcile(operation_id)
        }
    };
    run_collection_command(
        command,
        CollectionContext {
            service_directory: args.service_directory,
            json: args.json,
        },
    )
}
