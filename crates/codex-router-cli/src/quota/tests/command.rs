use std::ffi::OsString;
use std::path::Path;

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
async fn quota_async_entry_owns_help_and_reset_migration_guidance() {
    let mut stdout = Vec::new();
    run_quota_command(
        &mut stdout,
        QuotaCommand::Help("quota help\n"),
        false,
        false,
        None,
    )
    .await
    .expect("help dispatch");
    assert_eq!(stdout, b"quota help\n");

    for (stdin_is_terminal, stdout_is_terminal) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let mut reset_stdout = Vec::new();
        run_quota_command(
            &mut reset_stdout,
            QuotaCommand::Reset,
            stdin_is_terminal,
            stdout_is_terminal,
            None,
        )
        .await
        .expect("reset migration guidance");
        assert_eq!(
            reset_stdout,
            b"Quota reset moved to codex-router quota: focus an account and press Ctrl-R.\n"
        );
    }
}

#[test]
fn quota_reset_parser_accepts_only_migration_guidance_and_help_aliases() {
    for help_alias in ["--help", "-h", "help"] {
        let mut parser = ArgumentParser::new(vec!["reset".into(), help_alias.into()]);
        let QuotaCommand::Help(help_text) =
            QuotaCommand::parse(&mut parser).expect("reset help alias")
        else {
            panic!("reset help alias must use the ordinary help path");
        };
        assert!(help_text.contains("focus an account and press Ctrl-R"));
    }

    for rejected_arguments in [
        vec!["reset".into(), "--router-root".into(), "/tmp/router".into()],
        vec!["reset".into(), "--unknown".into()],
        vec!["reset".into(), "extra".into()],
    ] {
        let mut parser = ArgumentParser::new(rejected_arguments);
        assert!(matches!(
            QuotaCommand::parse(&mut parser),
            Err(CliError::UnknownOption { .. })
        ));
    }
}

#[test]
fn installed_quota_parser_rejects_harness_listener_before_dispatch() {
    let result = crate::CliCommand::parse([
        OsString::from("codex-router"),
        OsString::from("quota"),
        OsString::from("--provider-listener"),
        OsString::from("127.0.0.1:9"),
        OsString::from("--router-root"),
        OsString::from("/fixture-must-not-be-opened"),
    ]);

    assert!(matches!(result, Err(CliError::UnknownOption { .. })));
}

#[tokio::test]
async fn non_interactive_dispatch_never_constructs_an_injected_reset_session() {
    struct RejectingResetSessionFactory;

    impl crate::quota_reset::InteractiveResetSessionFactory for RejectingResetSessionFactory {
        fn create(
            &self,
            _router_root: &Path,
        ) -> Result<crate::quota_reset::InteractiveResetSession, crate::quota_reset::QuotaResetError>
        {
            panic!("non-interactive quota dispatch must not construct a reset session");
        }
    }

    for command in [QuotaCommand::Help("quota help\n"), QuotaCommand::Reset] {
        run_quota_command_with_reset_session_factory(
            &mut Vec::new(),
            command,
            false,
            false,
            None,
            &RejectingResetSessionFactory,
        )
        .await
        .expect("non-interactive dispatch");
    }
}
