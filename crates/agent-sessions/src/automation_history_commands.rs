//! Separate CLI entrypoints for delivery evidence and immutable instruction revisions.
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
    /// List durable obligations, optionally restricted to an exact wake-up.
    List(DeliveryListOptions),
    /// Inspect pending, accepted or uncertain delivery evidence without sending it again.
    Show {
        #[arg(long)]
        delivery_id: String,
    },
}
pub fn run_delivery_command(arguments: Vec<OsString>) -> i32 {
    let args = match DeliveryArguments::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
        }
    };
    let command = match args.command {
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
    let args = match RevisionArguments::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
        }
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
