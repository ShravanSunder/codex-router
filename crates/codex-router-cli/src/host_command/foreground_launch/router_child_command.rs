//! Exact child projection for the loopback Router process.

use std::ffi::OsString;
use std::path::PathBuf;

use codex_router_host::ChildCommandSpec;
use codex_router_host::ChildOutput;

use crate::CliContext;

use super::HostLaunchMode;

pub(super) struct RouterChildCommandInputs<'a> {
    pub(super) executable: PathBuf,
    pub(super) router_root: PathBuf,
    pub(super) port: u16,
    pub(super) launch_mode: HostLaunchMode,
    pub(super) otlp_endpoint: String,
    pub(super) context: &'a CliContext,
}

pub(super) fn router_child_command(inputs: RouterChildCommandInputs<'_>) -> ChildCommandSpec {
    let RouterChildCommandInputs {
        executable,
        router_root,
        port,
        launch_mode,
        otlp_endpoint,
        context,
    } = inputs;
    let arguments = vec![
        OsString::from("serve"),
        OsString::from("--port"),
        OsString::from(port.to_string()),
        OsString::from("--state-db"),
        router_root.join("state.sqlite").into_os_string(),
        OsString::from("--secret-root"),
        router_root.join("secrets").into_os_string(),
    ];
    #[cfg(debug_assertions)]
    let arguments = {
        let mut arguments = arguments;
        if launch_mode.is_isolated() {
            arguments.push(OsString::from("--require-debug-isolation"));
            if let Some(claude_upstream_base_url) =
                context.env_var("CODEX_ROUTER_DEBUG_CLAUDE_UPSTREAM_BASE_URL")
            {
                arguments.push(OsString::from("--debug-claude-upstream-base-url"));
                arguments.push(OsString::from(claude_upstream_base_url));
            }
        }
        arguments
    };
    #[cfg(not(debug_assertions))]
    let arguments = {
        let _ = launch_mode;
        arguments
    };

    let command = ChildCommandSpec::new(executable)
        .with_arguments(arguments)
        .with_environment("OTEL_EXPORTER_OTLP_ENDPOINT", otlp_endpoint)
        .with_environment("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .with_output(ChildOutput::Telemetry);
    #[cfg(not(debug_assertions))]
    let _ = (launch_mode, context);

    command
}

#[cfg(test)]
mod tests {
    use super::{RouterChildCommandInputs, router_child_command};
    use crate::CliContext;
    use codex_router_host::ChildCommandSpec;
    use std::{ffi::OsString, path::PathBuf};

    #[test]
    fn isolated_router_child_uses_configured_port_and_passes_claude_endpoint_flag() {
        let executable = PathBuf::from("/tmp/codex-router");
        let router_root = PathBuf::from("/tmp/private-router");
        let context = CliContext::new(vec![(
            "CODEX_ROUTER_DEBUG_CLAUDE_UPSTREAM_BASE_URL".to_owned(),
            "http://127.0.0.1:18888".to_owned(),
        )]);
        let command = router_child_command(RouterChildCommandInputs {
            executable: executable.clone(),
            router_root: router_root.clone(),
            port: 19876,
            launch_mode: super::super::HostLaunchMode::IsolatedDebug,
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
            context: &context,
        });

        #[cfg(debug_assertions)]
        let expected_arguments = [
            OsString::from("serve"),
            OsString::from("--port"),
            OsString::from("19876"),
            OsString::from("--state-db"),
            router_root.join("state.sqlite").into_os_string(),
            OsString::from("--secret-root"),
            router_root.join("secrets").into_os_string(),
            OsString::from("--require-debug-isolation"),
            OsString::from("--debug-claude-upstream-base-url"),
            OsString::from("http://127.0.0.1:18888"),
        ];
        #[cfg(not(debug_assertions))]
        let expected_arguments = [
            OsString::from("serve"),
            OsString::from("--port"),
            OsString::from("19876"),
            OsString::from("--state-db"),
            router_root.join("state.sqlite").into_os_string(),
            OsString::from("--secret-root"),
            router_root.join("secrets").into_os_string(),
        ];
        let expected = ChildCommandSpec::new(executable)
            .with_arguments(expected_arguments)
            .with_environment("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318")
            .with_environment("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
            .with_output(codex_router_host::ChildOutput::Telemetry);

        assert_eq!(command, expected);
    }
}
