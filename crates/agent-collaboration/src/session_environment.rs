//! Environment and local endpoint selection for the Sessions executable.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliContext {
    env: Vec<(String, String)>,
    current_dir: PathBuf,
}
impl CliContext {
    #[must_use]
    pub fn new(env: Vec<(String, String)>) -> Self {
        Self {
            env,
            current_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
    #[must_use]
    pub fn with_current_dir(mut self, current_dir: PathBuf) -> Self {
        self.current_dir = current_dir;
        self
    }
    pub(crate) fn env_var(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }
    pub(crate) fn current_dir(&self) -> &Path {
        &self.current_dir
    }
}
pub(crate) fn app_server_socket_or_default(
    context: &CliContext,
    paths: &codex_native_integration::CodexPaths,
) -> Result<PathBuf, &'static str> {
    codex_native_integration::select_app_server_endpoint(
        codex_native_integration::AppServerEndpointSelection {
            paths,
            debug_defaults: cfg!(all(debug_assertions, not(test)))
                && context.env_var("CODEX_ROUTER_USE_HOME_DEFAULT").is_none(),
            requested_socket: context.env_var("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET"),
        },
    )
}
pub fn run_arguments(arguments: Vec<OsString>) -> Result<(), String> {
    let command = crate::sessions::SessionsCommand::parse(arguments)?;
    let context = CliContext::new(std::env::vars().collect());
    crate::sessions::run_sessions_command(&mut std::io::stdout(), command, &context)
        .map_err(|error| error.to_string())
}
