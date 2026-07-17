use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use super::*;

static ASYNC_REFRESH_ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

struct IsolatedAsyncRefreshRoot {
    path: PathBuf,
}

impl IsolatedAsyncRefreshRoot {
    fn new() -> Self {
        let sequence = ASYNC_REFRESH_ROOT_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "codex-router-async-refresh-{}-{sequence}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).expect("remove stale async refresh fixture root");
        }
        fs::create_dir_all(&path).expect("create isolated async refresh fixture root");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IsolatedAsyncRefreshRoot {
    fn drop(&mut self) {
        if self.path.exists() {
            fs::remove_dir_all(&self.path).expect("remove isolated async refresh fixture root");
        }
    }
}

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
    let component_source = include_str!("../presentation/quota/quota_status_component.rs");

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
        include_str!("quota_command_dispatch.rs"),
        include_str!("quota_refresh_command.rs"),
        include_str!("quota_refresh_history.rs"),
        include_str!("quota_refresh_service.rs"),
        include_str!("quota_status_command.rs"),
        include_str!("quota_status_loader.rs"),
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
fn quota_refresh_composition_uses_only_the_async_credential_resolver() {
    let refresh_command_source = include_str!("quota_refresh_command.rs");
    assert!(
        refresh_command_source
            .contains("crate::credential_runtime::AsyncCliCredentialResolver::open("),
        "production quota refresh must construct the native-async credential resolver"
    );
    assert!(
        !refresh_command_source.contains("let resolver = CliCredentialResolver::open("),
        "production quota refresh must not regress to the sync resolver"
    );

    let credential_runtime_source = include_str!("../credential_runtime.rs");
    let async_resolver_source = credential_runtime_source
        .split_once("pub(crate) struct AsyncCliCredentialResolver")
        .and_then(|(_before, source)| {
            source
                .split_once("impl<C> AsyncProviderCredentialResolver for CliCredentialResolver")
                .map(|(async_resolver, _after)| async_resolver)
        })
        .expect("native-async resolver source boundary");
    for forbidden_runtime_owner in [
        "tokio::runtime::Builder",
        "tokio::runtime::Runtime",
        ".block_on(",
    ] {
        assert!(
            !async_resolver_source.contains(forbidden_runtime_owner),
            "native-async resolver must not own {forbidden_runtime_owner}"
        );
    }
}

#[test]
fn top_level_async_dispatch_routes_every_quota_variant_before_sync_fallback() {
    let cli_source = include_str!("../lib.rs");
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

#[tokio::test]
async fn production_quota_refresh_composition_runs_inside_the_process_runtime() {
    let router_root = IsolatedAsyncRefreshRoot::new();
    let mut stdout = Vec::new();

    refresh_quota(
        &mut stdout,
        router_root.path().to_path_buf(),
        DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned(),
    )
    .await
    .expect("empty isolated refresh should complete without provider egress");

    assert_eq!(stdout, b"refreshed: 0\n");
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

#[tokio::test]
async fn reset_session_join_failure_is_a_sanitized_command_error() {
    // Arrange
    let session_task = tokio::spawn(std::future::pending::<
        crate::quota_reset::reset_session_supervisor::ResetSessionOutcome,
    >());
    session_task.abort();

    // Act
    let error = await_reset_session_task(session_task)
        .await
        .expect_err("session panic must fail the command");

    // Assert
    assert!(matches!(error, QuotaCommandError::ResetSessionTaskFailed));
    assert_eq!(
        error.to_string(),
        "integrated quota reset session task failed"
    );
}
