fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1
        && arguments
            .first()
            .is_some_and(|arg| arg == "--version" || arg == "-V")
    {
        println!("agent-collaboration {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if arguments.len() == 1
        && arguments
            .first()
            .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        print!("{}", agent_collaboration::command_help());
        return;
    }
    if let Some(argument) = arguments.first().and_then(|argument| argument.to_str())
        && argument.starts_with("board ")
    {
        let message = "The board command was passed as one argument. Pass each word as a separate argument, for example: agent-collaboration board project list --json";
        if argument
            .split_ascii_whitespace()
            .any(|word| word == "--json")
            || arguments
                .iter()
                .skip(1)
                .any(|argument| argument == "--json")
        {
            println!(
                "{}",
                serde_json::json!({"kind":"error","error":{"kind":"invalidUsage","stage":"validation","message":message,"nextAction":"correctRequest","details":{"kind":"none"}}})
            );
        } else {
            eprintln!("{message}");
        }
        std::process::exit(2);
    }
    if arguments.first().is_some_and(|arg| arg == "board") {
        std::process::exit(agent_collaboration::run_board_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "automation") {
        std::process::exit(agent_collaboration::run_automation_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "delivery") {
        std::process::exit(agent_collaboration::run_delivery_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "revision") {
        std::process::exit(agent_collaboration::run_revision_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "operation") {
        std::process::exit(agent_collaboration::run_operation_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "run") {
        std::process::exit(agent_collaboration::run_workflow_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "schedule") {
        std::process::exit(agent_collaboration::run_schedule_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "wake") {
        std::process::exit(agent_collaboration::run_wakeup_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "instruction") {
        std::process::exit(agent_collaboration::run_instruction_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "conversation") {
        std::process::exit(agent_collaboration::run_conversation_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "approval") {
        std::process::exit(agent_collaboration::run_approval_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "sessions") {
        std::process::exit(agent_collaboration::run_session_inventory_command(
            arguments,
        ));
    }
    if arguments.first().is_some_and(|arg| arg == "events") {
        std::process::exit(agent_collaboration::run_event_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "message") {
        std::process::exit(agent_collaboration::run_message_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "native") {
        std::process::exit(agent_collaboration::run_native_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "acp") {
        std::process::exit(agent_collaboration::run_acp_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "whoami") {
        std::process::exit(agent_collaboration::run_whoami_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "endpoints") {
        std::process::exit(agent_collaboration::run_endpoint_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "addresses") {
        std::process::exit(agent_collaboration::run_address_command(arguments));
    }
    if arguments.first().is_some_and(|arg| arg == "journal") {
        std::process::exit(agent_collaboration::run_journal_command(arguments));
    }
    if arguments
        .first()
        .is_some_and(|arg| arg == "session" || arg == "turn")
    {
        std::process::exit(agent_collaboration::run_native_session_command(arguments));
    }
    if let Err(error) = agent_collaboration::run_arguments(arguments) {
        eprintln!("{error}");
        std::process::exit(error.exit_code());
    }
}
