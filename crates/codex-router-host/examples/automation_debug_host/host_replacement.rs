//! Re-exec support for the isolated acceptance Host.

use super::launch_options::DebugHostOptions;
use codex_router_host::{ChildCommandSpec, ChildOutput, HostCoordinationPaths, HostInstance};

pub(super) fn build_host_replacement_command(
    options: &DebugHostOptions,
) -> Result<ChildCommandSpec, std::io::Error> {
    let arguments = options.resume_arguments();
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
