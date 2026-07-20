//! Non-publishable compiled entry for hermetic quota-reset PTY proof.

fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--sessions-picker")) {
        std::process::exit(codex_router_cli::run_sessions_picker_test_harness());
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    std::process::exit(runtime.block_on(codex_router_cli::run_quota_reset_test_harness()));
}
