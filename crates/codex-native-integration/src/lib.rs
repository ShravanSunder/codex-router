//! Version-bounded integration with the managed upstream Codex executable.

mod app_server_launch;
mod desktop_launch_policy;
mod native_executable_identity;
mod native_protocol_observation;
mod native_session_launch;
mod native_state_paths;
mod remote_control_observation;
mod router_profile_projection;

pub use app_server_launch::AppServerCommandSpec;
pub use desktop_launch_policy::DesktopLaunchPolicyCommand;
pub use desktop_launch_policy::DesktopLaunchPolicyError;
pub use native_executable_identity::ExecutableIdentity;
pub use native_executable_identity::ExecutableIdentityError;
pub use native_executable_identity::ExecutableIdentityTask;
pub use native_executable_identity::UpdaterCommandSpec;
pub use native_executable_identity::executable_identity;
pub use native_executable_identity::managed_executable_version;
pub use native_executable_identity::start_executable_identity;
pub use native_protocol_observation::AppServerObservation;
pub use native_protocol_observation::CodexProtocolError;
pub use native_protocol_observation::observe_app_server;
pub use native_session_launch::SessionLaunch;
pub use native_session_launch::SessionProfile;
pub use native_state_paths::CodexPaths;
pub use remote_control_observation::RemoteControlObservation;
pub use router_profile_projection::CodexRouterProfile;

mod session_search_expression;
pub use session_search_expression::SessionSearchDocument;
pub use session_search_expression::SessionSearchExpression;
