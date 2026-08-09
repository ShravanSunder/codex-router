//! Direct interactive-session attachment projection.

use std::ffi::OsString;
use std::path::Path;

/// Root Codex arguments for a direct native app-server attachment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionLaunch {
    arguments: Vec<OsString>,
}

impl SessionLaunch {
    /// Builds arguments for a new interactive session.
    #[must_use]
    pub fn new(socket_path: &Path, user_arguments: &[OsString]) -> Self {
        Self {
            arguments: root_arguments(socket_path, user_arguments),
        }
    }

    /// Builds arguments for a new local interactive session.
    #[must_use]
    pub fn local(user_arguments: &[OsString]) -> Self {
        Self {
            arguments: local_root_arguments(user_arguments),
        }
    }

    /// Builds arguments for resuming one interactive session.
    #[must_use]
    pub fn resume(socket_path: &Path, user_arguments: &[OsString], session_id: &str) -> Self {
        let mut arguments = root_arguments(socket_path, user_arguments);
        arguments.extend([
            OsString::from("resume"),
            OsString::from("--"),
            OsString::from(session_id),
        ]);
        Self { arguments }
    }

    /// Builds arguments for locally resuming one interactive session.
    #[must_use]
    pub fn resume_local(user_arguments: &[OsString], session_id: &str) -> Self {
        let mut arguments = local_root_arguments(user_arguments);
        arguments.extend([
            OsString::from("resume"),
            OsString::from("--"),
            OsString::from(session_id),
        ]);
        Self { arguments }
    }

    /// Returns the projected root arguments.
    #[must_use]
    pub fn arguments(&self) -> Vec<OsString> {
        self.arguments.clone()
    }
}

fn root_arguments(socket_path: &Path, user_arguments: &[OsString]) -> Vec<OsString> {
    let mut arguments = local_root_arguments(&[]);
    arguments.extend([
        OsString::from("--remote"),
        OsString::from(format!("unix://{}", socket_path.display())),
    ]);
    arguments.extend_from_slice(user_arguments);
    arguments
}

fn local_root_arguments(user_arguments: &[OsString]) -> Vec<OsString> {
    let mut arguments = vec![OsString::from("--profile"), OsString::from("codex-router")];
    arguments.extend_from_slice(user_arguments);
    arguments
}
