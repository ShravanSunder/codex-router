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

/// Model and reasoning effort a stored session last ran with.
///
/// Resuming through the Router profile otherwise applies the profile defaults, which
/// silently replaces the model a long-running session was chosen for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResumeModelChoice {
    model: Option<String>,
    reasoning_effort: Option<String>,
}

impl ResumeModelChoice {
    /// Builds a choice from stored catalog values, dropping values that cannot be
    /// expressed as a TOML basic string.
    #[must_use]
    pub fn from_stored_values(model: Option<&str>, reasoning_effort: Option<&str>) -> Self {
        Self {
            model: stored_value(model),
            reasoning_effort: stored_value(reasoning_effort),
        }
    }

    /// Reports whether a stored value was dropped because it cannot be quoted safely.
    #[must_use]
    pub fn rejects_stored_value(model: Option<&str>, reasoning_effort: Option<&str>) -> bool {
        [model, reasoning_effort]
            .into_iter()
            .flatten()
            .map(str::trim)
            .any(|value| !value.is_empty() && !is_toml_basic_string_safe(value))
    }

    fn overrides_for(&self, caller: CallerOverrides) -> Vec<OsString> {
        let mut arguments = Vec::new();
        if !caller.model
            && let Some(model) = &self.model
        {
            arguments.push(OsString::from("-c"));
            arguments.push(OsString::from(format!("model=\"{model}\"")));
        }
        if !caller.reasoning_effort
            && let Some(effort) = &self.reasoning_effort
        {
            arguments.push(OsString::from("-c"));
            arguments.push(OsString::from(format!(
                "model_reasoning_effort=\"{effort}\""
            )));
        }
        arguments
    }
}

/// Keeps a stored value only when it is present and can be quoted safely.
fn stored_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && is_toml_basic_string_safe(value))
        .map(str::to_owned)
}

/// A TOML basic string cannot carry an unescaped quote or backslash.
fn is_toml_basic_string_safe(value: &str) -> bool {
    !value.contains(['"', '\\']) && !value.chars().any(char::is_control)
}

/// Which model configuration keys the caller's own Codex arguments already set.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CallerOverrides {
    pub model: bool,
    pub reasoning_effort: bool,
}

/// Reports which model keys the caller already set, so a stored choice never wins over
/// an explicit argument.
#[must_use]
pub fn caller_overrides(codex_args: &[OsString]) -> CallerOverrides {
    let mut overrides = CallerOverrides::default();
    let mut expects_model_value = false;
    let mut expects_config_value = false;
    for argument in codex_args {
        let Some(value) = argument.to_str() else {
            expects_model_value = false;
            expects_config_value = false;
            continue;
        };
        if expects_model_value {
            expects_model_value = false;
            overrides.model = true;
            continue;
        }
        if expects_config_value {
            expects_config_value = false;
            note_config_assignment(&mut overrides, value);
            continue;
        }
        match value {
            "-m" | "--model" => expects_model_value = true,
            "-c" | "--config" => expects_config_value = true,
            _ => note_attached_argument(&mut overrides, value),
        }
    }
    overrides
}

fn note_attached_argument(overrides: &mut CallerOverrides, value: &str) {
    if let Some(rest) = value.strip_prefix("--model=") {
        if !rest.is_empty() {
            overrides.model = true;
        }
    } else if let Some(rest) = value.strip_prefix("--config=") {
        note_config_assignment(overrides, rest);
    } else if let Some(rest) = value.strip_prefix("-m") {
        if !attached_value(rest).is_empty() {
            overrides.model = true;
        }
    } else if let Some(rest) = value.strip_prefix("-c") {
        note_config_assignment(overrides, attached_value(rest));
    }
}

fn attached_value(rest: &str) -> &str {
    rest.strip_prefix('=').unwrap_or(rest)
}

fn note_config_assignment(overrides: &mut CallerOverrides, assignment: &str) {
    let Some((key, _value)) = assignment.split_once('=') else {
        return;
    };
    match key.trim() {
        "model" => overrides.model = true,
        "model_reasoning_effort" => overrides.reasoning_effort = true,
        _ => {}
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
        model_choice: &ResumeModelChoice,
    ) -> Self {
        let mut arguments = root_arguments(
            socket_path,
            invoking_cwd,
            &hosted_resume_arguments(user_arguments),
        );
        arguments.extend(model_choice.overrides_for(caller_overrides(user_arguments)));
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
        model_choice: &ResumeModelChoice,
    ) -> Self {
        let mut arguments = local_root_arguments(invoking_cwd, user_arguments);
        arguments.extend(model_choice.overrides_for(caller_overrides(user_arguments)));
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
        model_choice: &ResumeModelChoice,
    ) -> Self {
        let mut arguments = root_arguments(
            socket_path,
            invoking_cwd,
            &hosted_resume_arguments(user_arguments),
        );
        arguments.extend(model_choice.overrides_for(caller_overrides(user_arguments)));
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
    pub fn fork_local(
        invoking_cwd: &Path,
        user_arguments: &[OsString],
        session_id: &str,
        model_choice: &ResumeModelChoice,
    ) -> Self {
        let mut arguments = local_root_arguments(invoking_cwd, user_arguments);
        arguments.extend(model_choice.overrides_for(caller_overrides(user_arguments)));
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
