//! Native launch target and profile selection for the Sessions product.

use std::ffi::OsString;
use std::path::PathBuf;

use super::SessionsCommand;
use super::SessionsCommandError;
use super::codex_home;
use super::normalize_path;
use crate::CliContext;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SessionsLaunchTarget {
    Hosted {
        app_server_socket: PathBuf,
        service_directory: PathBuf,
        invoking_cwd: PathBuf,
        profile: codex_native_integration::SessionProfile,
    },
    Local {
        invoking_cwd: PathBuf,
        profile: codex_native_integration::SessionProfile,
    },
}

impl SessionsLaunchTarget {
    pub(super) fn resolve_for_launch(&mut self) -> Result<(), SessionsCommandError> {
        if let Self::Hosted {
            app_server_socket,
            service_directory,
            ..
        } = self
        {
            *app_server_socket = communication_client::resolve_public_native(service_directory)
                .map_err(SessionsCommandError::CodexLaunch)?;
        }
        Ok(())
    }
    fn profile(&self) -> codex_native_integration::SessionProfile {
        match self {
            Self::Hosted { profile, .. } | Self::Local { profile, .. } => *profile,
        }
    }

    pub(super) fn new_launch(
        &self,
        codex_args: &[OsString],
    ) -> codex_native_integration::SessionLaunch {
        match self {
            Self::Hosted {
                app_server_socket,
                invoking_cwd,
                ..
            } => codex_native_integration::SessionLaunch::new(
                app_server_socket,
                invoking_cwd,
                codex_args,
            ),
            Self::Local { invoking_cwd, .. } => {
                codex_native_integration::SessionLaunch::local(invoking_cwd, codex_args)
            }
        }
        .with_profile(self.profile())
    }

    pub(super) fn resume_launch(
        &self,
        codex_args: &[OsString],
        session_id: &str,
    ) -> codex_native_integration::SessionLaunch {
        match self {
            Self::Hosted {
                app_server_socket,
                invoking_cwd,
                ..
            } => codex_native_integration::SessionLaunch::resume(
                app_server_socket,
                invoking_cwd,
                codex_args,
                session_id,
            ),
            Self::Local { invoking_cwd, .. } => {
                codex_native_integration::SessionLaunch::resume_local(
                    invoking_cwd,
                    codex_args,
                    session_id,
                )
            }
        }
        .with_profile(self.profile())
    }

    pub(super) fn fork_launch(
        &self,
        codex_args: &[OsString],
        session_id: &str,
    ) -> codex_native_integration::SessionLaunch {
        match self {
            Self::Hosted {
                app_server_socket,
                invoking_cwd,
                ..
            } => codex_native_integration::SessionLaunch::fork(
                app_server_socket,
                invoking_cwd,
                codex_args,
                session_id,
            ),
            Self::Local { invoking_cwd, .. } => {
                codex_native_integration::SessionLaunch::fork_local(
                    invoking_cwd,
                    codex_args,
                    session_id,
                )
            }
        }
        .with_profile(self.profile())
    }
}

pub(super) fn sessions_launch_target(
    command: &SessionsCommand,
    context: &CliContext,
) -> Result<SessionsLaunchTarget, SessionsCommandError> {
    let invoking_cwd = normalize_path(context.current_dir());
    let profile = session_profile_for_environment(
        cfg!(all(debug_assertions, not(test))),
        context.env_var("CODEX_ROUTER_USE_HOME_DEFAULT").is_some(),
    );
    if command.local {
        return Ok(SessionsLaunchTarget::Local {
            invoking_cwd,
            profile,
        });
    }
    let codex_paths = codex_native_integration::CodexPaths::from_codex_home(codex_home(context)?);
    let _validated_backend = crate::app_server_socket_or_default(context, &codex_paths)
        .map_err(|message| SessionsCommandError::AppServerSocket(message.to_owned()))?;
    let debug = cfg!(all(debug_assertions, not(test)))
        && context.env_var("CODEX_ROUTER_USE_HOME_DEFAULT").is_none();
    let selected_root = if debug {
        context
            .env_var("CODEX_ROUTER_DEBUG_ROUTER_ROOT")
            .map(PathBuf::from)
    } else {
        None
    };
    let router_root = match selected_root {
        Some(root) => root,
        None => PathBuf::from(context.env_var("HOME").ok_or_else(|| {
            SessionsCommandError::AppServerSocket(
                "HOME required for communication discovery".into(),
            )
        })?)
        .join(if debug {
            ".codex-router-debug"
        } else {
            ".codex-router"
        }),
    };
    if !router_root.is_absolute() {
        return Err(SessionsCommandError::AppServerSocket(
            "Router root must be absolute".into(),
        ));
    }
    let service_directory = router_root.join("agent-communication");
    let app_server_socket = service_directory.join("codex-native.sock");
    Ok(SessionsLaunchTarget::Hosted {
        app_server_socket,
        service_directory,
        invoking_cwd,
        profile,
    })
}

pub(super) fn session_profile_for_environment(
    debug_defaults: bool,
    use_home_default: bool,
) -> codex_native_integration::SessionProfile {
    if debug_defaults && !use_home_default {
        codex_native_integration::SessionProfile::RouterDebug
    } else {
        codex_native_integration::SessionProfile::Router
    }
}
