//! Privacy-bounded classification of managed child stderr.

use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::ChildStderr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChildDiagnosticClass {
    OauthRefreshRejected,
    ModelCatalogSchemaMismatch,
    RemoteControlFailure,
    Unclassified,
}

impl ChildDiagnosticClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::OauthRefreshRejected => "oauth_refresh_rejected",
            Self::ModelCatalogSchemaMismatch => "model_catalog_schema_mismatch",
            Self::RemoteControlFailure => "remote_control_failure",
            Self::Unclassified => "unclassified",
        }
    }
}

pub(crate) fn classify_child_stderr(line: &str) -> ChildDiagnosticClass {
    let normalized = line.to_ascii_lowercase();
    if normalized.contains("invalid_grant")
        && (normalized.contains("oauth") || normalized.contains("refresh token"))
    {
        ChildDiagnosticClass::OauthRefreshRejected
    } else if normalized.contains("failed to refresh available models")
        && normalized.contains("missing field")
    {
        ChildDiagnosticClass::ModelCatalogSchemaMismatch
    } else if normalized.contains("remote control") && normalized.contains("error") {
        ChildDiagnosticClass::RemoteControlFailure
    } else {
        ChildDiagnosticClass::Unclassified
    }
}

pub(crate) fn spawn_child_stderr_reader(source: &'static str, stderr: ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let diagnostic_class = classify_child_stderr(&line);
            tracing::warn!(
                event.name = "codex_router.host.child_diagnostic",
                child.source = source,
                error.kind = diagnostic_class.as_str(),
                "managed child emitted stderr"
            );
            diagnostic_counter().add(
                1,
                &[
                    KeyValue::new("child.source", source),
                    KeyValue::new("error.kind", diagnostic_class.as_str()),
                ],
            );
        }
    });
}

fn diagnostic_counter() -> &'static Counter<u64> {
    static COUNTER: OnceLock<Counter<u64>> = OnceLock::new();
    COUNTER.get_or_init(|| {
        opentelemetry::global::meter("codex-router-host")
            .u64_counter("codex_router_host_child_diagnostic_total")
            .with_description("Count of scrubbed managed-child stderr diagnostics")
            .build()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_server_diagnostics_classify_known_failures_without_retaining_raw_lines() {
        assert_eq!(
            classify_child_stderr(
                "OAuth refresh token was rejected: invalid_grant: Grant not found"
            ),
            ChildDiagnosticClass::OauthRefreshRejected
        );
        assert_eq!(
            classify_child_stderr(
                "failed to refresh available models: missing field `display_name`"
            ),
            ChildDiagnosticClass::ModelCatalogSchemaMismatch
        );
        assert_eq!(
            classify_child_stderr("a diagnostic with SECRET_CANARY payload"),
            ChildDiagnosticClass::Unclassified
        );
    }
}
