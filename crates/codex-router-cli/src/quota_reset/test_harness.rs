//! Sealed parser and composition for the compiled quota-reset PTY harness.

use std::ffi::OsString;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;

use thiserror::Error;

use super::InteractiveResetSessionFactory;
use super::LoopbackInteractiveResetSessionFactory;
use crate::ArgumentParser;
use crate::CliCommand;
use crate::CliContext;
use crate::CliError;
use crate::quota::QuotaCommand;

const ROUTER_ROOT_OPTION: &str = "--router-root";
const PROVIDER_LISTENER_OPTION: &str = "--provider-listener";

#[derive(Debug, Error)]
pub(crate) enum QuotaResetTestHarnessError {
    #[error(transparent)]
    Cli(#[from] CliError),
    #[error("missing required harness option: {option}")]
    MissingOption { option: &'static str },
    #[error("duplicate harness option: {option}")]
    DuplicateOption { option: &'static str },
    #[error("fixture router root must be an absolute path")]
    RouterRootNotAbsolute,
    #[error("provider listener must be a numeric loopback SocketAddr with a nonzero port")]
    InvalidProviderListener,
    #[error("quota-reset harness synthesized a non-quota command")]
    InvalidSynthesizedCommand,
}

struct HarnessDispatch {
    command: QuotaCommand,
    provider_listener: SocketAddr,
}

impl HarnessDispatch {
    fn parse<I>(args: I) -> Result<Self, QuotaResetTestHarnessError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut process_arguments = args.into_iter();
        let _binary_name = process_arguments.next();
        let mut parser = ArgumentParser::new(process_arguments.collect());
        let mut router_root = None;
        let mut provider_listener = None;

        while let Some(option) = parser.next_string()? {
            match option.as_str() {
                ROUTER_ROOT_OPTION => {
                    if router_root.is_some() {
                        return Err(QuotaResetTestHarnessError::DuplicateOption {
                            option: ROUTER_ROOT_OPTION,
                        });
                    }
                    router_root = Some(PathBuf::from(
                        parser.next_required_value(ROUTER_ROOT_OPTION)?,
                    ));
                }
                PROVIDER_LISTENER_OPTION => {
                    if provider_listener.is_some() {
                        return Err(QuotaResetTestHarnessError::DuplicateOption {
                            option: PROVIDER_LISTENER_OPTION,
                        });
                    }
                    let value = parser.next_required_value(PROVIDER_LISTENER_OPTION)?;
                    provider_listener =
                        Some(value.parse::<SocketAddr>().map_err(|_error| {
                            QuotaResetTestHarnessError::InvalidProviderListener
                        })?);
                }
                _unknown => {
                    return Err(CliError::UnknownOption { option }.into());
                }
            }
        }

        let router_root = router_root.ok_or(QuotaResetTestHarnessError::MissingOption {
            option: ROUTER_ROOT_OPTION,
        })?;
        if !router_root.is_absolute() {
            return Err(QuotaResetTestHarnessError::RouterRootNotAbsolute);
        }
        let provider_listener =
            provider_listener.ok_or(QuotaResetTestHarnessError::MissingOption {
                option: PROVIDER_LISTENER_OPTION,
            })?;
        if !provider_listener.ip().is_loopback() || provider_listener.port() == 0 {
            return Err(QuotaResetTestHarnessError::InvalidProviderListener);
        }

        let command = CliCommand::parse([
            OsString::from("codex-router"),
            OsString::from("quota"),
            OsString::from(ROUTER_ROOT_OPTION),
            router_root.into_os_string(),
        ])?;
        let CliCommand::Quota(command) = command else {
            return Err(QuotaResetTestHarnessError::InvalidSynthesizedCommand);
        };

        Ok(Self {
            command,
            provider_listener,
        })
    }
}

pub(crate) async fn run_quota_reset_test_harness_with_io<I, W, E>(
    args: I,
    context: &CliContext,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), QuotaResetTestHarnessError>
where
    I: IntoIterator<Item = OsString>,
    W: Write,
    E: Write,
{
    run_with_factory_builder(
        args,
        context,
        stdout,
        stderr,
        LoopbackInteractiveResetSessionFactory::new,
    )
    .await
}

async fn run_with_factory_builder<I, W, E, TFactory, TFactoryBuilder>(
    args: I,
    context: &CliContext,
    stdout: &mut W,
    stderr: &mut E,
    build_factory: TFactoryBuilder,
) -> Result<(), QuotaResetTestHarnessError>
where
    I: IntoIterator<Item = OsString>,
    W: Write,
    E: Write,
    TFactory: InteractiveResetSessionFactory,
    TFactoryBuilder: FnOnce(SocketAddr) -> TFactory,
{
    let dispatch = HarnessDispatch::parse(args)?;
    let reset_session_factory = build_factory(dispatch.provider_listener);
    crate::quota::run_quota_command_with_reset_session_factory(
        stdout,
        dispatch.command,
        context.stdin_is_terminal(),
        context.stdout_is_terminal(),
        context.stdout_terminal_width(),
        &reset_session_factory,
    )
    .await
    .map_err(CliError::from)?;
    stderr.flush().map_err(CliError::Stderr)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::quota_reset::InteractiveResetSession;
    use crate::quota_reset::QuotaResetError;

    struct RejectingFactory;

    impl InteractiveResetSessionFactory for RejectingFactory {
        fn create(&self, _router_root: &Path) -> Result<InteractiveResetSession, QuotaResetError> {
            Err(QuotaResetError::Response {
                message: "test factory must not be called".to_owned(),
            })
        }
    }

    fn harness_args(router_root: &str, provider_listener: &str) -> Vec<OsString> {
        [
            "codex-router-quota-reset-test-harness",
            ROUTER_ROOT_OPTION,
            router_root,
            PROVIDER_LISTENER_OPTION,
            provider_listener,
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    #[test]
    fn parser_accepts_only_absolute_root_and_numeric_loopback_listener() {
        let dispatch = HarnessDispatch::parse(harness_args("/fixtures/router", "127.0.0.1:4321"))
            .expect("valid harness arguments");

        assert!(matches!(dispatch.command, QuotaCommand::Status { .. }));
        assert_eq!(
            dispatch.provider_listener,
            "127.0.0.1:4321".parse::<SocketAddr>().expect("address")
        );
    }

    #[tokio::test]
    async fn invalid_inputs_are_rejected_before_factory_construction() {
        for args in [
            harness_args("relative/router", "127.0.0.1:4321"),
            harness_args("/fixtures/router", "example.test:4321"),
            harness_args("/fixtures/router", "192.0.2.1:4321"),
            harness_args("/fixtures/router", "127.0.0.1:0"),
        ] {
            let constructions = Arc::new(AtomicUsize::new(0));
            let factory_constructions = Arc::clone(&constructions);
            let result = run_with_factory_builder(
                args,
                &CliContext::new(Vec::new()),
                &mut Vec::new(),
                &mut Vec::new(),
                move |_provider_listener| {
                    factory_constructions.fetch_add(1, Ordering::SeqCst);
                    RejectingFactory
                },
            )
            .await;

            assert!(result.is_err());
            assert_eq!(constructions.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn missing_duplicate_and_unknown_options_fail_closed() {
        let missing = HarnessDispatch::parse([OsString::from("harness")]);
        assert!(matches!(
            missing,
            Err(QuotaResetTestHarnessError::MissingOption {
                option: ROUTER_ROOT_OPTION
            })
        ));

        let duplicate = HarnessDispatch::parse([
            OsString::from("harness"),
            OsString::from(ROUTER_ROOT_OPTION),
            OsString::from("/first"),
            OsString::from(ROUTER_ROOT_OPTION),
            OsString::from("/second"),
        ]);
        assert!(matches!(
            duplicate,
            Err(QuotaResetTestHarnessError::DuplicateOption {
                option: ROUTER_ROOT_OPTION
            })
        ));

        let unknown =
            HarnessDispatch::parse([OsString::from("harness"), OsString::from("--provider-url")]);
        assert!(matches!(
            unknown,
            Err(QuotaResetTestHarnessError::Cli(
                CliError::UnknownOption { .. }
            ))
        ));
    }
}
