fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1
        && arguments
            .first()
            .is_some_and(|arg| arg == "--version" || arg == "-V")
    {
        println!("agent-sessions {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if arguments.len() == 1
        && arguments
            .first()
            .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        print!("{}", agent_sessions::command_help());
        return;
    }
    if arguments.first().is_some_and(|arg| arg == "automation") {
        std::process::exit(agent_sessions::run_automation_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "delivery") {
        std::process::exit(agent_sessions::run_delivery_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "revision") {
        std::process::exit(agent_sessions::run_revision_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "run") {
        std::process::exit(agent_sessions::run_workflow_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "schedule") {
        std::process::exit(agent_sessions::run_schedule_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "wake") {
        std::process::exit(agent_sessions::run_wakeup_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "instruction") {
        std::process::exit(agent_sessions::run_instruction_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "conversation") {
        std::process::exit(agent_sessions::run_conversation_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "sessions") {
        std::process::exit(agent_sessions::run_session_inventory_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "events") {
        std::process::exit(agent_sessions::run_event_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "message") {
        std::process::exit(agent_sessions::run_message_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "native") {
        std::process::exit(agent_sessions::run_native_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "acp") {
        std::process::exit(agent_sessions::run_acp_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "endpoints") {
        std::process::exit(agent_sessions::run_endpoint_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "addresses") {
        std::process::exit(agent_sessions::run_address_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "journal") {
        std::process::exit(agent_sessions::run_journal_command(arguments));
    }
    if arguments
        .first()
        .is_some_and(|arg| arg == "session" || arg == "turn")
    {
        std::process::exit(agent_sessions::run_native_session_command(arguments));
    }
    if let Err(error) = agent_sessions::run_arguments(arguments) {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
