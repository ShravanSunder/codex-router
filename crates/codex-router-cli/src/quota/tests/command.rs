use super::*;

#[test]
fn interactive_quota_requires_effective_table_format_and_both_terminals() {
    for format in [
        QuotaStatusFormat::Table,
        QuotaStatusFormat::Plain,
        QuotaStatusFormat::Json,
    ] {
        for stdin_is_terminal in [false, true] {
            for stdout_is_terminal in [false, true] {
                let expected =
                    format == QuotaStatusFormat::Table && stdin_is_terminal && stdout_is_terminal;
                assert_eq!(
                    should_run_interactive_quota(format, stdin_is_terminal, stdout_is_terminal,),
                    expected,
                    "format={format:?}, stdin={stdin_is_terminal}, stdout={stdout_is_terminal}"
                );
            }
        }
    }
}

#[test]
fn interactive_quota_component_uses_the_process_async_runtime() {
    let component_source = include_str!("../../presentation/quota/component.rs");

    for forbidden_runtime_owner in ["tokio::runtime::Builder", ".block_on(", "spawn_blocking"] {
        assert!(
            !component_source.contains(forbidden_runtime_owner),
            "interactive quota component must not own {forbidden_runtime_owner}"
        );
    }
}

#[test]
fn quota_command_modules_keep_runtime_wrappers_in_background_worker_only() {
    let quota_command_sources = concat!(
        include_str!("../command.rs"),
        include_str!("../refresh_command.rs"),
        include_str!("../refresh_history.rs"),
        include_str!("../refresh_service.rs"),
        include_str!("../status_command.rs"),
        include_str!("../status_loader.rs"),
    );

    for forbidden_runtime_owner in [
        "tokio::runtime::Builder",
        ".block_on(",
        "thread::spawn",
        "std::thread::spawn",
    ] {
        assert!(
            !quota_command_sources.contains(forbidden_runtime_owner),
            "quota command call graph must not own {forbidden_runtime_owner}"
        );
    }
}

#[test]
fn top_level_async_dispatch_routes_every_quota_variant_before_sync_fallback() {
    let cli_source = include_str!("../../lib.rs");
    let async_entry = cli_source
        .split("pub async fn run_async()")
        .nth(1)
        .and_then(|source| {
            source
                .split("pub async fn run_quota_reset_test_harness")
                .next()
        })
        .expect("process async entry source");

    assert!(
        async_entry.contains("Ok(CliCommand::Quota(_))"),
        "every parsed quota variant must select native async dispatch"
    );
    assert!(
        !async_entry.contains("QuotaCommand::Status")
            && !async_entry.contains("QuotaCommand::Refresh")
            && !async_entry.contains("QuotaCommand::Reset"),
        "top-level runtime selection must not special-case quota variants"
    );
}

#[tokio::test]
async fn quota_async_entry_owns_help_and_legacy_reset_dispatch() {
    let mut stdout = Vec::new();
    let help_dispatch = run_quota_command(
        &mut stdout,
        QuotaCommand::Help("quota help\n"),
        false,
        false,
        None,
    )
    .await
    .expect("help dispatch");
    assert_eq!(help_dispatch, QuotaCommandDispatch::Complete);
    assert_eq!(stdout, b"quota help\n");

    let router_root = PathBuf::from("/unused-reset-dispatch-root");
    let reset_dispatch = run_quota_command(
        &mut stdout,
        QuotaCommand::Reset {
            router_root: router_root.clone(),
        },
        false,
        false,
        None,
    )
    .await
    .expect("reset dispatch");
    assert_eq!(
        reset_dispatch,
        QuotaCommandDispatch::LegacyReset { router_root }
    );
}
