//! Sealed parser and composition for the compiled quota-reset PTY harness.

use std::ffi::OsString;
use std::fs;
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
const FIXTURE_CAPABILITY_OPTION: &str = "--fixture-capability";
const PROVIDER_LISTENER_OPTION: &str = "--provider-listener";
const FIXTURE_ROOT_MARKER_NAME: &str = ".codex-router-quota-reset-test-fixture";
const FIXTURE_ROOT_MARKER_PREFIX: &str = "codex-router-quota-reset-test-fixture:v1:";

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
    #[error(
        "fixture root capability must be 16-128 ASCII letters, digits, hyphens, or underscores"
    )]
    InvalidFixtureCapability,
    #[error("fixture router root could not be canonicalized")]
    RouterRootUnavailable,
    #[error("fixture router root must be a canonical child of the process temporary directory")]
    RouterRootOutsideTemporaryDirectory,
    #[error("fixture router root is missing its regular-file capability marker")]
    MissingFixtureRootMarker,
    #[error("fixture router root capability marker does not match the supplied capability")]
    InvalidFixtureRootMarker,
    #[error("provider listener must be a numeric loopback SocketAddr with a nonzero port")]
    InvalidProviderListener,
    #[error("quota-reset harness synthesized a non-quota command")]
    InvalidSynthesizedCommand,
}

struct HarnessDispatch {
    command: QuotaCommand,
    provider_listener: SocketAddr,
}

struct FixtureRootCapability {
    canonical_root: PathBuf,
}

impl FixtureRootCapability {
    fn authorize(
        router_root: PathBuf,
        supplied_capability: &str,
    ) -> Result<Self, QuotaResetTestHarnessError> {
        if !router_root.is_absolute() {
            return Err(QuotaResetTestHarnessError::RouterRootNotAbsolute);
        }
        if !valid_fixture_capability(supplied_capability) {
            return Err(QuotaResetTestHarnessError::InvalidFixtureCapability);
        }
        let canonical_root = fs::canonicalize(router_root)
            .map_err(|_error| QuotaResetTestHarnessError::RouterRootUnavailable)?;
        let canonical_temporary_directory = fs::canonicalize(std::env::temp_dir())
            .map_err(|_error| QuotaResetTestHarnessError::RouterRootUnavailable)?;
        if canonical_root == canonical_temporary_directory
            || !canonical_root.starts_with(&canonical_temporary_directory)
        {
            return Err(QuotaResetTestHarnessError::RouterRootOutsideTemporaryDirectory);
        }
        let marker_path = canonical_root.join(FIXTURE_ROOT_MARKER_NAME);
        let marker_metadata = fs::symlink_metadata(&marker_path)
            .map_err(|_error| QuotaResetTestHarnessError::MissingFixtureRootMarker)?;
        if !marker_metadata.file_type().is_file() {
            return Err(QuotaResetTestHarnessError::MissingFixtureRootMarker);
        }
        let marker_bytes = fs::read(marker_path)
            .map_err(|_error| QuotaResetTestHarnessError::InvalidFixtureRootMarker)?;
        let expected_marker = format!("{FIXTURE_ROOT_MARKER_PREFIX}{supplied_capability}\n");
        if marker_bytes != expected_marker.as_bytes() {
            return Err(QuotaResetTestHarnessError::InvalidFixtureRootMarker);
        }
        Ok(Self { canonical_root })
    }
}

fn valid_fixture_capability(candidate: &str) -> bool {
    (16..=128).contains(&candidate.len())
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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
        let mut fixture_capability = None;
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
                FIXTURE_CAPABILITY_OPTION => {
                    if fixture_capability.is_some() {
                        return Err(QuotaResetTestHarnessError::DuplicateOption {
                            option: FIXTURE_CAPABILITY_OPTION,
                        });
                    }
                    fixture_capability =
                        Some(parser.next_required_value(FIXTURE_CAPABILITY_OPTION)?);
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
        let fixture_capability =
            fixture_capability.ok_or(QuotaResetTestHarnessError::MissingOption {
                option: FIXTURE_CAPABILITY_OPTION,
            })?;
        let fixture_root = FixtureRootCapability::authorize(router_root, &fixture_capability)?;
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
            fixture_root.canonical_root.into_os_string(),
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
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::quota_reset::InteractiveResetSession;
    use crate::quota_reset::QuotaResetError;

    static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct IsolatedFixtureRoot {
        path: PathBuf,
        capability: String,
    }

    impl IsolatedFixtureRoot {
        fn create() -> Self {
            let sequence = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let capability = format!("unit-fixture-{}-{sequence}", std::process::id());
            let path = std::env::temp_dir().join(format!(
                "codex-router-quota-reset-harness-unit-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create isolated fixture root");
            fs::write(
                path.join(FIXTURE_ROOT_MARKER_NAME),
                format!("{FIXTURE_ROOT_MARKER_PREFIX}{capability}\n"),
            )
            .expect("write isolated fixture marker");
            Self { path, capability }
        }
    }

    impl Drop for IsolatedFixtureRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct RejectingFactory;

    impl InteractiveResetSessionFactory for RejectingFactory {
        fn create(&self, _router_root: &Path) -> Result<InteractiveResetSession, QuotaResetError> {
            Err(QuotaResetError::Response {
                message: "test factory must not be called".to_owned(),
            })
        }
    }

    fn harness_args(
        router_root: &Path,
        fixture_capability: &str,
        provider_listener: &str,
    ) -> Vec<OsString> {
        [
            OsString::from("codex-router-quota-reset-test-harness"),
            OsString::from(ROUTER_ROOT_OPTION),
            router_root.as_os_str().to_owned(),
            OsString::from(FIXTURE_CAPABILITY_OPTION),
            OsString::from(fixture_capability),
            OsString::from(PROVIDER_LISTENER_OPTION),
            OsString::from(provider_listener),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn parser_accepts_only_capability_marked_isolated_root_and_numeric_loopback_listener() {
        let fixture = IsolatedFixtureRoot::create();
        let dispatch = HarnessDispatch::parse(harness_args(
            &fixture.path,
            &fixture.capability,
            "127.0.0.1:4321",
        ))
        .expect("valid harness arguments");

        assert!(matches!(dispatch.command, QuotaCommand::Status { .. }));
        assert_eq!(
            dispatch.provider_listener,
            "127.0.0.1:4321".parse::<SocketAddr>().expect("address")
        );
    }

    #[tokio::test]
    async fn invalid_inputs_are_rejected_before_factory_construction() {
        let fixture = IsolatedFixtureRoot::create();
        for args in [
            harness_args(
                Path::new("relative/router"),
                &fixture.capability,
                "127.0.0.1:4321",
            ),
            harness_args(&fixture.path, "short", "127.0.0.1:4321"),
            harness_args(&fixture.path, &fixture.capability, "example.test:4321"),
            harness_args(&fixture.path, &fixture.capability, "192.0.2.1:4321"),
            harness_args(&fixture.path, &fixture.capability, "127.0.0.1:0"),
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

    #[tokio::test]
    async fn home_default_debug_and_ordinary_roots_are_rejected_before_factory_construction() {
        let ordinary_root = std::env::temp_dir().join(format!(
            "codex-router-ordinary-root-{}",
            FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&ordinary_root).expect("create ordinary root");
        let mut rejected_roots = vec![ordinary_root.clone()];
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            rejected_roots.push(home.clone());
            rejected_roots.push(home.join(".codex-router"));
            rejected_roots.push(home.join(".codex-router-debug"));
        }

        for rejected_root in rejected_roots {
            let constructions = Arc::new(AtomicUsize::new(0));
            let factory_constructions = Arc::clone(&constructions);
            let result = run_with_factory_builder(
                harness_args(&rejected_root, "ordinary-root-capability", "127.0.0.1:4321"),
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
        fs::remove_dir_all(ordinary_root).expect("remove ordinary root");
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
