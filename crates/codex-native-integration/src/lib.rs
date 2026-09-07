//! Version-bounded integration with the managed upstream Codex executable.

mod app_server_launch;
mod stored_thread_catalog;
mod stored_thread_query;
pub use stored_thread_catalog::StoredThreadCatalog;
pub use stored_thread_query::path_sql_values;
pub use stored_thread_query::{
    StoredThreadCursor, StoredThreadProvider, StoredThreadQuery, StoredThreadRoot,
    StoredThreadSort, StoredThreadSource, stored_thread_page_query,
};
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
mod native_protocol_connection;
pub use native_protocol_connection::{NativeConnectionError, NativeProtocolConnection};

mod native_input_submission;
mod native_payload_schemas;
mod native_schema_bundle;
mod native_schema_export;
mod native_schema_validation;
mod validated_native_request;
pub use native_payload_schemas::{NativeOperation, NativePayloadSchemas};
pub use native_schema_export::NativeSchemaExport;
pub use native_schema_validation::NativeSchemaValidator;
mod schema_bundle_storage;
mod schema_export_collection;
mod schema_json_validation;
pub use native_schema_bundle::{NativeSchemaBundle, NativeSchemaError};
mod native_thread_operations;
pub use native_input_submission::NativeInputSubmission;

mod debug_endpoint_selection;
pub use debug_endpoint_selection::{
    AppServerEndpointSelection, select_app_server_endpoint, validate_debug_directory,
    validate_debug_endpoint,
};

mod debug_profile_projection;
pub use debug_profile_projection::{DebugCodexProfile, DebugProfileError};
