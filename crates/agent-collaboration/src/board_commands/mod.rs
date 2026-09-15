//! Descriptive message-board commands over the public communication client.
mod board_arguments;
mod board_execution;
mod board_preparation;
mod board_search_commands;
mod board_thread_listen_execution;
mod board_value_parsing;

use board_arguments::BoardArguments;
use std::ffi::OsString;

/// Runs a board command without opening board storage in the CLI process.
pub fn run_board_command(arguments: Vec<OsString>) -> i32 {
    let machine = arguments.iter().any(|argument| argument == "--json");
    let parsed =
        match crate::automation_argument_feedback::parse_arguments::<BoardArguments>(arguments) {
            Ok(parsed) => parsed,
            Err(code) => return code,
        };
    let (command, context) = match board_preparation::prepare(parsed.command) {
        Ok(prepared) => prepared,
        Err(message) => {
            return crate::endpoint_commands::report_failure("invalidField", &message, 2, machine);
        }
    };
    match command {
        board_preparation::PreparedBoardCommand::ThreadListen(pending) => {
            board_thread_listen_execution::execute(pending, context)
        }
        board_preparation::PreparedBoardCommand::ThreadListenShow(request) => {
            board_thread_listen_execution::execute_show(request, context)
        }
        board_preparation::PreparedBoardCommand::ThreadListenCancel(request) => {
            board_thread_listen_execution::execute_cancel(request, context)
        }
        command => board_execution::execute(command, context),
    }
}
