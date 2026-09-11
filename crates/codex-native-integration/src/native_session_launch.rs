//! Direct interactive-session attachment projection.

use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Path;

/// Explicit configuration profile for native Codex launches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionProfile {
    /// Existing installed Router profile.
    Router,
    /// Isolated debug Router profile in normal Codex home.
    RouterDebug,
}

impl SessionProfile {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Router => "codex-router",
            Self::RouterDebug => "codex-router-debug",
        }
    }
}

/// Root Codex arguments for a direct native app-server attachment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionLaunch {
    profile: SessionProfile,
    arguments: Vec<OsString>,
}

impl SessionLaunch {
    /// Builds arguments for a new interactive session.
    #[must_use]
    pub fn new(socket_path: &Path, invoking_cwd: &Path, user_arguments: &[OsString]) -> Self {
        Self {
            profile: SessionProfile::Router,
            arguments: root_arguments(socket_path, invoking_cwd, user_arguments),
        }
    }

    /// Builds arguments for a new local interactive session.
    #[must_use]
    pub fn local(invoking_cwd: &Path, user_arguments: &[OsString]) -> Self {
        Self {
            profile: SessionProfile::Router,
            arguments: local_root_arguments(invoking_cwd, user_arguments),
        }
    }

    /// Builds arguments for resuming one interactive session.
    #[must_use]
    pub fn resume(
        socket_path: &Path,
        invoking_cwd: &Path,
        user_arguments: &[OsString],
        session_id: &str,
    ) -> Self {
        let mut arguments = root_arguments(
            socket_path,
            invoking_cwd,
            &hosted_resume_arguments(user_arguments),
        );
        arguments.extend([
            OsString::from("resume"),
            OsString::from("--"),
            OsString::from(session_id),
        ]);
        Self {
            profile: SessionProfile::Router,
            arguments,
        }
    }

    /// Builds arguments for locally resuming one interactive session.
    #[must_use]
    pub fn resume_local(
        invoking_cwd: &Path,
        user_arguments: &[OsString],
        session_id: &str,
    ) -> Self {
        let mut arguments = local_root_arguments(invoking_cwd, user_arguments);
        arguments.extend([
            OsString::from("resume"),
            OsString::from("--"),
            OsString::from(session_id),
        ]);
        Self {
            profile: SessionProfile::Router,
            arguments,
        }
    }

    /// Builds arguments for forking one interactive session.
    #[must_use]
    pub fn fork(
        socket_path: &Path,
        invoking_cwd: &Path,
        user_arguments: &[OsString],
        session_id: &str,
    ) -> Self {
        let mut arguments = root_arguments(
            socket_path,
            invoking_cwd,
            &hosted_resume_arguments(user_arguments),
        );
        arguments.extend([
            OsString::from("fork"),
            OsString::from("--"),
            OsString::from(session_id),
        ]);
        Self {
            profile: SessionProfile::Router,
            arguments,
        }
    }

    /// Builds arguments for locally forking one interactive session.
    #[must_use]
    pub fn fork_local(invoking_cwd: &Path, user_arguments: &[OsString], session_id: &str) -> Self {
        let mut arguments = local_root_arguments(invoking_cwd, user_arguments);
        arguments.extend([
            OsString::from("fork"),
            OsString::from("--"),
            OsString::from(session_id),
        ]);
        Self {
            profile: SessionProfile::Router,
            arguments,
        }
    }

    /// Selects the launcher-owned Codex configuration profile.
    #[must_use]
    pub const fn with_profile(mut self, profile: SessionProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Returns the projected root arguments.
    #[must_use]
    pub fn arguments(&self) -> Vec<OsString> {
        [
            OsString::from("--profile"),
            OsString::from(self.profile.name()),
        ]
        .into_iter()
        .chain(self.arguments.iter().cloned())
        .collect()
    }
}

fn root_arguments(
    socket_path: &Path,
    invoking_cwd: &Path,
    user_arguments: &[OsString],
) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("--remote"),
        OsString::from(format!("unix://{}", socket_path.display())),
    ];
    append_default_working_directory(&mut arguments, invoking_cwd, user_arguments);
    arguments.extend_from_slice(user_arguments);
    arguments
}

fn local_root_arguments(invoking_cwd: &Path, user_arguments: &[OsString]) -> Vec<OsString> {
    let mut arguments = Vec::new();
    append_default_working_directory(&mut arguments, invoking_cwd, user_arguments);
    arguments.extend_from_slice(user_arguments);
    arguments
}

fn hosted_resume_arguments(user_arguments: &[OsString]) -> Vec<OsString> {
    let mut result = Vec::with_capacity(user_arguments.len());
    let mut skip_value = false;
    for argument in user_arguments {
        if skip_value {
            skip_value = false;
            continue;
        }
        let Some(value) = argument.to_str() else {
            result.push(argument.clone());
            continue;
        };
        if matches!(value, "--sandbox" | "-s" | "--ask-for-approval" | "-a") {
            skip_value = true;
            continue;
        }
        if matches!(
            value,
            "--yolo" | "--dangerously-bypass-approvals-and-sandbox" | "--approve-for-me"
        ) || value.starts_with("--sandbox=")
            || value.starts_with("--ask-for-approval=")
            || value.starts_with("-c")
                && (value.contains("approval_policy")
                    || value.contains("sandbox_mode")
                    || value.contains("sandbox_permissions"))
        {
            continue;
        }
        result.push(argument.clone());
    }
    result
}

fn append_default_working_directory(
    arguments: &mut Vec<OsString>,
    invoking_cwd: &Path,
    user_arguments: &[OsString],
) {
    if user_arguments.iter().any(|argument| {
        let encoded_argument = argument.as_encoded_bytes();
        argument == OsStr::new("--cd")
            || argument == OsStr::new("-C")
            || encoded_argument.starts_with(b"--cd=")
            || (encoded_argument.starts_with(b"-C") && encoded_argument.len() > 2)
    }) {
        return;
    }
    arguments.extend([OsString::from("--cd"), invoking_cwd.as_os_str().to_owned()]);
}
