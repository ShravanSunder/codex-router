#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LoopbackConnectionDiagnostic {
    class: &'static str,
    safe_reason: &'static str,
    severity: &'static str,
}

impl LoopbackConnectionDiagnostic {
    const fn new(class: &'static str, safe_reason: &'static str, severity: &'static str) -> Self {
        Self {
            class,
            safe_reason,
            severity,
        }
    }

    #[cfg(test)]
    const fn class(self) -> &'static str {
        self.class
    }

    #[cfg(test)]
    const fn safe_reason(self) -> &'static str {
        self.safe_reason
    }

    #[cfg(test)]
    const fn severity(self) -> &'static str {
        self.severity
    }

    fn render(self) -> String {
        format!(
            "codex-router loopback connection failed: severity={} class={} reason={}",
            self.severity, self.class, self.safe_reason
        )
    }
}

fn loopback_connection_diagnostic(
    error: &LoopbackRouterRuntimeError,
) -> LoopbackConnectionDiagnostic {
    match error {
        LoopbackRouterRuntimeError::HyperConnection(source)
        | LoopbackRouterRuntimeError::HyperBody(source) => hyper_loopback_error_diagnostic(source),
        LoopbackRouterRuntimeError::ConnectionJoin(_) => {
            LoopbackConnectionDiagnostic::new("join_failure", "task_join", "error")
        }
        LoopbackRouterRuntimeError::WebSocket(_) => LoopbackConnectionDiagnostic::new(
            "upstream_tunnel_failure",
            websocket_runtime_error_kind(error),
            "error",
        ),
        _ => LoopbackConnectionDiagnostic::new(
            "router_runtime_failure",
            websocket_runtime_error_kind(error),
            "error",
        ),
    }
}

fn hyper_loopback_error_diagnostic(error: &hyper::Error) -> LoopbackConnectionDiagnostic {
    if error.is_incomplete_message() {
        return LoopbackConnectionDiagnostic::new(
            "client_disconnect",
            "hyper_incomplete_message",
            "debug",
        );
    }
    if error.is_canceled() {
        return LoopbackConnectionDiagnostic::new("client_disconnect", "hyper_canceled", "debug");
    }
    if error.is_closed() {
        return LoopbackConnectionDiagnostic::new("client_disconnect", "hyper_closed", "debug");
    }
    if error.is_body_write_aborted() {
        return LoopbackConnectionDiagnostic::new(
            "client_disconnect",
            "hyper_body_write_aborted",
            "debug",
        );
    }
    if error.is_shutdown() {
        return LoopbackConnectionDiagnostic::new("client_disconnect", "hyper_shutdown", "debug");
    }
    if hyper_error_source_chain_contains(error, "end of file before message length reached") {
        return LoopbackConnectionDiagnostic::new(
            "client_disconnect",
            "hyper_incomplete_message",
            "debug",
        );
    }
    if error.is_parse() {
        return LoopbackConnectionDiagnostic::new("malformed_request", "hyper_parse", "warn");
    }
    if error.is_timeout() {
        return LoopbackConnectionDiagnostic::new("client_disconnect", "hyper_timeout", "debug");
    }

    LoopbackConnectionDiagnostic::new("unknown", "hyper_unknown", "error")
}

fn hyper_error_source_chain_contains(error: &hyper::Error, needle: &str) -> bool {
    let mut source = std::error::Error::source(error);
    while let Some(error_source) = source {
        if error_source.to_string().contains(needle) {
            return true;
        }
        source = error_source.source();
    }

    false
}
