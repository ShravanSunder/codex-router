//! Accepted debug launch options and their existing Host configuration projection.
use codex_router_host::{
    ChildCommandSpec, ChildOutput, ExternalProviderLaunchBinding, ExternalProviderStartup,
    HostConfig,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProviderCommand {
    pub(super) executable: PathBuf,
    pub(super) arguments: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RunDirectoryAdmission {
    Fresh,
    Resume,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DebugHostOptions {
    pub(super) run_directory: PathBuf,
    pub(super) run_directory_admission: RunDirectoryAdmission,
    pub(super) router_binary: PathBuf,
    pub(super) port: u16,
    pub(super) router_root: Option<PathBuf>,
    pub(super) claude_provider: Option<ProviderCommand>,
}

pub(super) fn parse_options() -> Result<Option<DebugHostOptions>, Box<dyn std::error::Error>> {
    parse_options_from(std::env::args_os().skip(1))
}

pub(super) fn parse_options_from(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<DebugHostOptions>, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let mut run_directory = None::<(PathBuf, RunDirectoryAdmission)>;
    let mut router_binary = None;
    let mut port = 18787;
    let mut router_root = None;
    let mut claude_executable = None;
    let mut claude_arguments = Vec::new();
    while let Some(flag) = arguments.next() {
        if flag == "--help" || flag == "-h" {
            println!(
                "automation-debug-host (--run-directory /tmp/NEW-DIRECTORY | --resume-run-directory /tmp/OWNED-DIRECTORY) --router-binary /ABSOLUTE/target/debug/codex-router [--port 18787] [--router-root /ABSOLUTE/ISOLATED-ROOT] [--claude-acp-executable /ABSOLUTE/ACP --claude-acp-arguments ARG ...]\n\nOpt-in real acceptance Host. Uses the existing codex-router-debug profile with an in-memory Luna override. Fresh mode creates runtime/workspace directories. Resume mode reopens only a stopped Host's validated private directory. Starts only its own debug router and app-server and shuts its children down on SIGTERM. Never use production paths or ports. Keep this process running while the acceptance runner connects."
            );
            return Ok(None);
        }
        let value = arguments.next().ok_or("option value missing; use --help")?;
        match flag.to_str() {
            Some("--run-directory") => select_run_directory(
                &mut run_directory,
                PathBuf::from(value),
                RunDirectoryAdmission::Fresh,
            )?,
            Some("--resume-run-directory") => select_run_directory(
                &mut run_directory,
                PathBuf::from(value),
                RunDirectoryAdmission::Resume,
            )?,
            Some("--router-binary") => router_binary = Some(PathBuf::from(value)),
            Some("--router-root") => router_root = Some(PathBuf::from(value)),
            Some("--claude-acp-executable") => claude_executable = Some(PathBuf::from(value)),
            Some("--claude-acp-arguments") => claude_arguments.push(
                value
                    .into_string()
                    .map_err(|_| "ACP argument is not UTF-8")?,
            ),
            Some("--port") => port = value.to_str().ok_or("port is not UTF-8")?.parse()?,
            _ => return Err("Unknown option; use --help".into()),
        }
    }
    let (run_directory, run_directory_admission) = run_directory.ok_or(
        "exactly one of --run-directory or --resume-run-directory is required; use --help",
    )?;
    let claude_provider = match claude_executable {
        Some(executable) => {
            if !executable.is_absolute()
                || !router_root
                    .as_ref()
                    .is_some_and(|root: &PathBuf| root.is_absolute())
            {
                return Err(
                    "ACP proof requires an absolute executable and explicit absolute --router-root"
                        .into(),
                );
            }
            Some(ProviderCommand {
                executable,
                arguments: claude_arguments,
            })
        }
        None if !claude_arguments.is_empty() => {
            return Err("--claude-acp-arguments requires --claude-acp-executable".into());
        }
        None => None,
    };
    if router_root.as_ref().is_some_and(|root| !root.is_absolute()) {
        return Err("--router-root must be absolute".into());
    }
    Ok(Some(DebugHostOptions {
        run_directory,
        run_directory_admission,
        router_binary: router_binary.ok_or("--router-binary required; use --help")?,
        port,
        router_root,
        claude_provider,
    }))
}

fn select_run_directory(
    selected: &mut Option<(PathBuf, RunDirectoryAdmission)>,
    run_directory: PathBuf,
    admission: RunDirectoryAdmission,
) -> Result<(), &'static str> {
    if selected.is_some() {
        return Err("--run-directory and --resume-run-directory are mutually exclusive");
    }
    *selected = Some((run_directory, admission));
    Ok(())
}

impl DebugHostOptions {
    pub(super) fn resume_arguments(&self) -> Vec<OsString> {
        let mut arguments = vec![
            OsString::from("--resume-run-directory"),
            self.run_directory.as_os_str().to_owned(),
            OsString::from("--router-binary"),
            self.router_binary.as_os_str().to_owned(),
            OsString::from("--port"),
            OsString::from(self.port.to_string()),
        ];
        if let Some(root) = &self.router_root {
            arguments.extend([OsString::from("--router-root"), root.as_os_str().to_owned()]);
        }
        if let Some(provider) = &self.claude_provider {
            arguments.extend([
                OsString::from("--claude-acp-executable"),
                provider.executable.as_os_str().to_owned(),
            ]);
            for argument in &provider.arguments {
                arguments.extend([
                    OsString::from("--claude-acp-arguments"),
                    OsString::from(argument),
                ]);
            }
        }
        arguments
    }

    pub(super) fn router_child_command(&self, router_root: &Path) -> ChildCommandSpec {
        ChildCommandSpec::new(self.router_binary.clone())
            .with_arguments([
                OsString::from("serve"),
                OsString::from("--require-debug-isolation"),
                OsString::from("--port"),
                OsString::from(self.port.to_string()),
                OsString::from("--state-db"),
                router_root.join("state.sqlite").into_os_string(),
                OsString::from("--secret-root"),
                router_root.join("secrets").into_os_string(),
                OsString::from("--audit-file"),
                self.run_directory
                    .join("router-audit.jsonl")
                    .into_os_string(),
            ])
            .with_output(ChildOutput::Telemetry)
    }

    pub(super) fn attach_provider(
        &self,
        config: HostConfig,
        global_claude_executable: Option<PathBuf>,
    ) -> Result<HostConfig, Box<dyn std::error::Error>> {
        let Some(provider) = &self.claude_provider else {
            return Ok(config);
        };
        let mut binding = ExternalProviderLaunchBinding::claude(
            provider.executable.clone(),
            provider.arguments.clone(),
        )?;
        let executable = global_claude_executable
            .ok_or("ACP proof requires the existing global CLAUDE_CODE_EXECUTABLE selector")?;
        if !executable.is_absolute() {
            return Err("global Claude selector must be absolute".into());
        }
        let executable = executable
            .to_str()
            .ok_or("global Claude selector is not UTF-8")?
            .to_owned();
        binding
            .launch
            .environment
            .push(("CLAUDE_CODE_EXECUTABLE".to_owned(), executable));
        Ok(config.with_external_provider_startup(ExternalProviderStartup::Launch(binding)))
    }
}
