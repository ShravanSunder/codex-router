//! Re-exec support for the isolated acceptance Host.

use codex_router_host::{ChildCommandSpec, ChildOutput, HostCoordinationPaths, HostInstance};
use std::{ffi::OsString, path::Path};

pub(super) fn build_host_replacement_command(
    run_directory: &Path,
    router_binary: &Path,
    port: u16,
) -> Result<ChildCommandSpec, std::io::Error> {
    let arguments = [
        OsString::from("--resume-run-directory"),
        run_directory.as_os_str().to_owned(),
        OsString::from("--router-binary"),
        router_binary.as_os_str().to_owned(),
        OsString::from("--port"),
        OsString::from(port.to_string()),
    ];
    Ok(ChildCommandSpec::new(std::env::current_exe()?)
        .with_arguments(arguments)
        .with_output(ChildOutput::Telemetry))
}

pub(super) fn acquire_acceptance_host_instance(
    paths: HostCoordinationPaths,
) -> Result<HostInstance, Box<dyn std::error::Error>> {
    let instance = match std::env::var_os(codex_router_host::inherited_lock_environment()) {
        Some(marker) => HostInstance::acquire_inherited(paths, &marker)?,
        None => HostInstance::acquire(paths)?,
    };
    Ok(instance)
}

pub(super) fn is_inherited_host_replacement(previous_host_pid: u32) -> bool {
    previous_host_pid == std::process::id()
        && std::env::var_os(codex_router_host::inherited_lock_environment()).is_some()
}
