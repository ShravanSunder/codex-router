//! Standalone terminal session picker.

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1
        && arguments
            .first()
            .is_some_and(|argument| argument == "--version" || argument == "-V")
    {
        println!("agent-session {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if arguments.len() == 1
        && arguments
            .first()
            .is_some_and(|argument| argument == "--help" || argument == "-h")
    {
        print!("{}", agent_collaboration::session_command_help());
        return;
    }
    if let Err(error) = agent_collaboration::run_arguments(arguments) {
        eprintln!("{error}");
        std::process::exit(error.exit_code());
    }
}
