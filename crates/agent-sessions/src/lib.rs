//! Independent Sessions picker and native launch product.
mod session_environment;
pub use session_environment::CliContext;
use session_environment::app_server_socket_or_default;
pub use session_environment::run_arguments;
#[path = "session_command_dispatch.rs"]
mod sessions;
mod presentation {
    pub(crate) use crate::session_picker;
}
mod session_picker;
#[cfg(feature = "quota-reset-test-harness")]
pub fn run_sessions_picker_test_harness() -> std::io::Result<()> {
    session_picker::run_sessions_picker_test_harness()
}

pub use sessions::SessionsCommandError;

/// Public executable usage generated from its argument parser.
#[must_use]
pub fn command_help() -> String {
    format!(
        "{}\nCommunication:\n  conversation prompt --endpoint ID --new|--session ID --cwd PATH --text-file PATH --json\n  sessions list --endpoint ID --view stored|loaded|active --json\n  events listen --endpoint ID --session ID --attach [--timeout-seconds 60]\n  message send --to ADDRESS --from ADDRESS [--delivery auto|queue|steer] --text TEXT --json\n  message send --human-user --to ADDRESS --text-file PATH --json\n  endpoints list --json [--service-directory PATH]\n  addresses list --endpoint ID --json [--cursor CURSOR]\n  session inspect --endpoint ID --session ID --json\n  turn interrupt --endpoint ID --session ID --turn ID --json\n  native --endpoint ID\n  acp --endpoint ID\n  journal status --json\n  journal read --endpoint ID --journal-id UUID --after SEQUENCE --json\n",
        sessions::command_help()
    )
}

mod endpoint_commands;
pub use endpoint_commands::run_endpoint_command;
mod address_book_commands;
pub use address_book_commands::run_address_command;

mod journal_read_commands;
mod native_stdio_bridge;
pub use journal_read_commands::run_journal_command;
pub use native_stdio_bridge::run_native_command;
mod native_session_commands;
pub use native_session_commands::run_native_session_command;

mod message_commands;
pub use message_commands::run_message_command;

mod event_commands;
pub use event_commands::run_event_command;

mod session_inventory_commands;
pub use session_inventory_commands::run_session_inventory_command;

mod conversation_commands;
pub use conversation_commands::run_conversation_command;

mod acp_stdio_bridge;
pub use acp_stdio_bridge::run_acp_command;

#[cfg(test)]
mod session_contract_tests;
