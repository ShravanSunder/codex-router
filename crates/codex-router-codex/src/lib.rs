//! Version-bounded integration with the managed upstream Codex executable.

mod app_server_control_protocol;
mod app_server_launch;
mod executable;
mod paths;
mod profile;
mod remote_control_observation;
mod session;

pub use app_server_control_protocol::AppServerObservation;
pub use app_server_control_protocol::CodexProtocolError;
pub use app_server_control_protocol::observe_app_server;
pub use app_server_launch::AppServerCommandSpec;
pub use executable::ExecutableIdentity;
pub use executable::ExecutableIdentityError;
pub use executable::ExecutableIdentityTask;
pub use executable::UpdaterCommandSpec;
pub use executable::executable_identity;
pub use executable::managed_executable_version;
pub use executable::start_executable_identity;
pub use paths::CodexPaths;
pub use profile::CodexRouterProfile;
pub use remote_control_observation::RemoteControlObservation;
pub use session::SessionLaunch;
