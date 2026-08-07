//! Version-bounded integration with the managed upstream Codex executable.

mod app_server;
mod executable;
mod paths;
mod profile;
mod protocol;
mod session;

pub use app_server::AppServerCommandSpec;
pub use executable::ExecutableIdentity;
pub use executable::ExecutableIdentityError;
pub use executable::ExecutableIdentityTask;
pub use executable::UpdaterCommandSpec;
pub use executable::executable_identity;
pub use executable::managed_executable_version;
pub use executable::start_executable_identity;
pub use paths::CodexPaths;
pub use profile::CodexRouterProfile;
pub use protocol::AppServerObservation;
pub use protocol::CodexProtocolError;
pub use protocol::RemoteControlObservation;
pub use protocol::observe_app_server;
pub use session::SessionLaunch;
