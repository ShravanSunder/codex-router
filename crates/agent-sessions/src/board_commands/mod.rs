//! Descriptive project-board commands over the public communication client.
mod board_arguments;
mod board_execution;
mod board_preparation;
mod board_value_parsing;

use board_arguments::BoardArguments;
use clap::Parser;
use std::ffi::OsString;

/// Runs a board command without opening board storage in the CLI process.
pub fn run_board_command(arguments: Vec<OsString>) -> i32 {
    let machine = arguments.iter().any(|argument| argument == "--json");
    let parsed = match BoardArguments::try_parse_from(arguments) {
        Ok(parsed) => parsed,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            return code;
        }
    };
    let (command, context) = match board_preparation::prepare(parsed.command) {
        Ok(prepared) => prepared,
        Err(message) => {
            return crate::endpoint_commands::report_failure("invalidField", &message, 2, machine);
        }
    };
    board_execution::execute(command, context)
}
