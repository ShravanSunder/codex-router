use collaboration_client::{ServiceDirectoryOptions, resolve_service_directory};
use std::{ffi::OsString, path::PathBuf};

fn options() -> ServiceDirectoryOptions {
    ServiceDirectoryOptions {
        explicit_directory: None,
        debug_defaults: false,
        use_home_default: false,
        debug_router_root: None,
        home_directory: Some(OsString::from("/Users/example")),
    }
}

#[test]
fn explicit_absolute_directory_has_priority_over_environment_inputs() {
    let mut inputs = options();
    inputs.explicit_directory = Some(PathBuf::from("/private/tmp/owned-service"));
    inputs.debug_defaults = true;
    inputs.debug_router_root = Some(OsString::from("relative-debug-root"));
    inputs.home_directory = None;

    assert_eq!(
        resolve_service_directory(inputs),
        Ok(PathBuf::from("/private/tmp/owned-service"))
    );
}

#[test]
fn debug_root_and_home_defaults_keep_the_literal_service_directory_name() {
    let mut debug_inputs = options();
    debug_inputs.debug_defaults = true;
    debug_inputs.debug_router_root = Some(OsString::from("/private/tmp/debug-router"));
    assert_eq!(
        resolve_service_directory(debug_inputs),
        Ok(PathBuf::from(
            "/private/tmp/debug-router/agent-communication"
        ))
    );

    let mut home_inputs = options();
    home_inputs.debug_defaults = true;
    home_inputs.use_home_default = true;
    assert_eq!(
        resolve_service_directory(home_inputs),
        Ok(PathBuf::from(
            "/Users/example/.codex-router/agent-communication"
        ))
    );

    let mut implicit_debug_inputs = options();
    implicit_debug_inputs.debug_defaults = true;
    assert_eq!(
        resolve_service_directory(implicit_debug_inputs),
        Ok(PathBuf::from(
            "/Users/example/.codex-router-debug/agent-communication"
        ))
    );
}

#[test]
fn relative_inputs_are_rejected_without_reading_process_environment() {
    let mut explicit_inputs = options();
    explicit_inputs.explicit_directory = Some(PathBuf::from("relative-service"));
    assert_eq!(
        resolve_service_directory(explicit_inputs).map_err(|error| error.to_string()),
        Err("service directory must be absolute".to_owned())
    );

    let mut home_inputs = options();
    home_inputs.home_directory = Some(OsString::from("relative-home"));
    assert_eq!(
        resolve_service_directory(home_inputs).map_err(|error| error.to_string()),
        Err("absolute HOME required".to_owned())
    );

    let mut debug_inputs = options();
    debug_inputs.debug_defaults = true;
    debug_inputs.debug_router_root = Some(OsString::from("relative-debug-root"));
    assert_eq!(
        resolve_service_directory(debug_inputs),
        Err(collaboration_client::ServiceDirectoryError::RelativeDebugRouterRoot)
    );
}
