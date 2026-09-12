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
mod picker_runtime_status;
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
        "{}\nCommunication:\n  board --help (projects, topics, messages, watches and inbox)\n  operation show --operation-id UUID --json\n  delivery attempts --delivery-id UUID [--cursor CURSOR] --json\n  run summaries --run-id UUID [--cursor CURSOR] --json\n  automation events [--after CURSOR] [--limit 50] --json\n  instruction list --json\n  schedule list --json\n  run list --schedule-id UUID --json\n  revision list --instruction-id UUID --json\n  delivery list [--wakeup-id UUID] --json\n  delivery show --delivery-id UUID --json\n  schedule export --schedule-id UUID > schedule.jsonl\n  schedule import --package-file PATH [--overwrite] --json\n  automation status --json\n  automation configure --execution-timeout-seconds 3600 --summary-timeout-seconds 900 --json\n  run show --run-id UUID --json\n  run summary-retry|summary-skip --run-id UUID [--operation-id UUID] --json\n  schedule create --definition-file PATH [--operation-id UUID] --json\n  schedule show --schedule-id UUID --json\n  schedule prepare --schedule-id UUID --fresh --cwd PATH --json\n  schedule update --schedule-id UUID --expected-change-id UUID --definition-file PATH --json\n  schedule enable|disable --schedule-id UUID --json\n  wake send --to ADDRESS --from ADDRESS --text TEXT --every 10m --for 2h [--wait-until-first-fire] --json\n  wake show --wakeup-id UUID --json\n  wake list [--limit 50] [--cursor CURSOR] --json\n  wake pause|resume|cancel --wakeup-id UUID [--operation-id UUID] --json\n  instruction create --text TEXT [--operation-id UUID] --json\n  instruction show --instruction-id UUID --json\n  instruction update --instruction-id UUID --expected-revision-id UUID --text TEXT --json\n  conversation prompt --endpoint ID --new|--session ID --cwd PATH --text-file PATH --json\n  sessions list --endpoint ID --view stored|loaded|active --json\n  events listen --endpoint ID --session ID --attach [--timeout-seconds 60]\n  message send --to ADDRESS --from ADDRESS [--delivery auto|queue|steer] --text TEXT --json\n  message send --human-user --to ADDRESS --text-file PATH --json\n  endpoints list --json [--service-directory PATH]\n  addresses list --endpoint ID --json [--cursor CURSOR]\n  session inspect --endpoint ID --session ID --json\n  turn interrupt --endpoint ID --session ID --turn ID --json\n  native --endpoint ID\n  acp --endpoint ID\n  journal status --json\n  journal read --endpoint ID --journal-id UUID --after SEQUENCE --json\n",
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

mod automation_argument_feedback;
mod automation_collection_commands;
mod automation_history_commands;
mod instruction_commands;
pub use automation_history_commands::{
    run_delivery_command, run_operation_command, run_revision_command,
};
pub use instruction_commands::run_instruction_command;

mod message_input_arguments;
mod wakeup_commands;
mod wakeup_timing_arguments;
pub use wakeup_commands::run_wakeup_command;

mod schedule_commands;
mod wakeup_wait_output;
pub use schedule_commands::run_schedule_command;

mod run_commands;
mod schedule_preparation_arguments;
pub use run_commands::run_workflow_command;
mod automation_configuration_commands;
pub use automation_configuration_commands::run_automation_command;

mod board_commands;
pub use board_commands::run_board_command;
