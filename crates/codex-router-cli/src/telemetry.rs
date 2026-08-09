//! Runtime telemetry setup for the codex-router CLI.

use std::env;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::Protocol;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use sha2::Digest;
use sha2::Sha256;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const DEFAULT_LOG_FILTER: &str = "warn,codex_router_cli=info,codex_router_proxy=info,opentelemetry_sdk=off,opentelemetry_otlp=off";
const SERVICE_NAME: &str = "codex-router";
const OTLP_ENDPOINT_ENV: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const OBSERVABILITY_MARKER_ENV: &str = "CODEX_ROUTER_OBSERVABILITY_MARKER";
const SHARED_LOCAL_OTLP_ENDPOINT: &str = "http://127.0.0.1:4318";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TelemetryMode {
    EnvironmentOnly,
    ForegroundHost,
}

#[derive(Debug)]
pub(crate) struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
    completed: Arc<AtomicBool>,
}

#[derive(Clone)]
pub(crate) struct TelemetryShutdownHandle {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
    completed: Arc<AtomicBool>,
}

impl TelemetryShutdownHandle {
    pub(crate) fn flush_and_shutdown(&self) {
        if self.completed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(tracer_provider) = &self.tracer_provider {
            let _ = tracer_provider.force_flush();
            let _ = tracer_provider.shutdown();
        }
        if let Some(meter_provider) = &self.meter_provider {
            let _ = meter_provider.force_flush();
            let _ = meter_provider.shutdown();
        }
        if let Some(logger_provider) = &self.logger_provider {
            let _ = logger_provider.force_flush();
            let _ = logger_provider.shutdown();
        }
    }
}

impl TelemetryGuard {
    pub(crate) fn shutdown_handle(&self) -> TelemetryShutdownHandle {
        TelemetryShutdownHandle {
            tracer_provider: self.tracer_provider.clone(),
            meter_provider: self.meter_provider.clone(),
            logger_provider: self.logger_provider.clone(),
            completed: self.completed.clone(),
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if self.completed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(tracer_provider) = self.tracer_provider.take() {
            let _ = tracer_provider.force_flush();
            let _ = tracer_provider.shutdown();
        }
        if let Some(meter_provider) = self.meter_provider.take() {
            let _ = meter_provider.force_flush();
            let _ = meter_provider.shutdown();
        }
        if let Some(logger_provider) = self.logger_provider.take() {
            let _ = logger_provider.force_flush();
            let _ = logger_provider.shutdown();
        }
    }
}

pub(crate) fn init_from_env(mode: TelemetryMode) -> TelemetryGuard {
    let filter = env::var("RUST_LOG").unwrap_or_else(|_error| DEFAULT_LOG_FILTER.to_owned());
    let Some(endpoint) = otlp_endpoint(mode) else {
        let _ = tracing_subscriber::registry()
            .with(EnvFilter::new(filter))
            .try_init();
        tracing::info!(
            service.name = SERVICE_NAME,
            service.version = env!("CARGO_PKG_VERSION"),
            "codex_router.process_start"
        );
        return TelemetryGuard {
            tracer_provider: None,
            meter_provider: None,
            logger_provider: None,
            completed: Arc::new(AtomicBool::new(false)),
        };
    };

    let tracer_provider_result = build_tracer_provider(&endpoint);
    let meter_provider_result = build_meter_provider(&endpoint);
    let logger_provider_result = build_logger_provider(&endpoint);
    let tracer_provider = tracer_provider_result.as_ref().ok().cloned();
    let meter_provider = meter_provider_result.as_ref().ok().cloned();
    if let Some(meter_provider) = meter_provider.clone() {
        global::set_meter_provider(meter_provider);
    }
    let logger_provider = logger_provider_result.as_ref().ok().cloned();
    let otel_layer = tracer_provider.as_ref().map(|provider| {
        let tracer = provider.tracer(SERVICE_NAME);
        tracing_opentelemetry::layer().with_tracer(tracer)
    });
    let log_layer = logger_provider
        .as_ref()
        .map(OpenTelemetryTracingBridge::new);
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::new(filter))
        .with(otel_layer)
        .with(log_layer)
        .try_init();
    record_exporter_initialization_failure(
        "traces",
        tracer_provider_result.as_ref().err().map(Box::as_ref),
    );
    record_exporter_initialization_failure(
        "metrics",
        meter_provider_result.as_ref().err().map(Box::as_ref),
    );
    record_exporter_initialization_failure(
        "logs",
        logger_provider_result.as_ref().err().map(Box::as_ref),
    );
    tracing::info!(
        service.name = SERVICE_NAME,
        service.version = env!("CARGO_PKG_VERSION"),
        otel.endpoint.configured = true,
        "codex_router.process_start"
    );

    TelemetryGuard {
        tracer_provider,
        meter_provider,
        logger_provider,
        completed: Arc::new(AtomicBool::new(false)),
    }
}

fn record_exporter_initialization_failure(
    signal: &'static str,
    error: Option<&(dyn std::error::Error + Send + Sync)>,
) {
    if let Some(error) = error {
        tracing::warn!(
            event.name = "codex_router.telemetry_exporter_initialization_failed",
            error.kind = "otel_exporter_initialization_failed",
            otel.signal = signal,
            error = %sanitize_error(&error.to_string()),
            "failed to initialize an OTLP exporter"
        );
    }
}

pub(crate) fn run_span() -> tracing::Span {
    tracing::info_span!(
        "codex_router.run",
        service.name = SERVICE_NAME,
        service.version = env!("CARGO_PKG_VERSION"),
        agent.proof.marker = observability_marker(),
    )
}

fn otlp_endpoint(mode: TelemetryMode) -> Option<String> {
    let explicit = env::var(OTLP_ENDPOINT_ENV).ok();
    resolve_otlp_endpoint(explicit.as_deref(), mode)
}

pub(crate) fn foreground_host_otlp_endpoint(explicit: Option<&str>) -> String {
    explicit
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .unwrap_or(SHARED_LOCAL_OTLP_ENDPOINT)
        .to_owned()
}

fn resolve_otlp_endpoint(explicit: Option<&str>, mode: TelemetryMode) -> Option<String> {
    explicit
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| match mode {
            TelemetryMode::EnvironmentOnly => None,
            TelemetryMode::ForegroundHost => Some(SHARED_LOCAL_OTLP_ENDPOINT.to_owned()),
        })
}

fn build_tracer_provider(
    endpoint: &str,
) -> Result<SdkTracerProvider, Box<dyn std::error::Error + Send + Sync>> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(trace_endpoint(endpoint))
        .with_protocol(Protocol::HttpBinary)
        .build()?;
    Ok(SdkTracerProvider::builder()
        .with_resource(telemetry_resource())
        .with_batch_exporter(exporter)
        .build())
}

fn build_meter_provider(
    endpoint: &str,
) -> Result<SdkMeterProvider, Box<dyn std::error::Error + Send + Sync>> {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(metric_endpoint(endpoint))
        .with_protocol(Protocol::HttpBinary)
        .build()?;
    Ok(SdkMeterProvider::builder()
        .with_resource(telemetry_resource())
        .with_periodic_exporter(exporter)
        .build())
}

fn build_logger_provider(
    endpoint: &str,
) -> Result<SdkLoggerProvider, Box<dyn std::error::Error + Send + Sync>> {
    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_endpoint(log_endpoint(endpoint))
        .with_protocol(Protocol::HttpBinary)
        .build()?;
    Ok(SdkLoggerProvider::builder()
        .with_resource(telemetry_resource())
        .with_batch_exporter(exporter)
        .build())
}

fn telemetry_resource() -> Resource {
    Resource::builder()
        .with_service_name(SERVICE_NAME)
        .with_attributes([
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new("dev.repo.hash", git_value_hash("--git-common-dir")),
            KeyValue::new("dev.worktree.hash", git_value_hash("--show-toplevel")),
            KeyValue::new("dev.branch.name", git_branch_name()),
            KeyValue::new("dev.runtime.flavor", runtime_flavor()),
            KeyValue::new("dev.release.channel", release_channel()),
            KeyValue::new("agent.proof.marker", observability_marker()),
        ])
        .build()
}

fn trace_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint.ends_with("/v1/traces") {
        endpoint.to_owned()
    } else {
        format!("{endpoint}/v1/traces")
    }
}

fn metric_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint.ends_with("/v1/metrics") {
        endpoint.to_owned()
    } else {
        format!("{endpoint}/v1/metrics")
    }
}

fn log_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint.ends_with("/v1/logs") {
        endpoint.to_owned()
    } else {
        format!("{endpoint}/v1/logs")
    }
}

fn git_value_hash(argument: &str) -> String {
    let value = Command::new("git")
        .args(["rev-parse", argument])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_owned())
        .filter(|stdout| !stdout.is_empty())
        .unwrap_or_else(|| {
            env::current_dir()
                .ok()
                .and_then(|path| path.to_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "unknown".to_owned())
        });
    stable_hash(&value)
}

fn git_branch_name() -> String {
    Command::new("git")
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_owned())
        .filter(|stdout| !stdout.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn stable_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn runtime_flavor() -> String {
    env::var("CODEX_ROUTER_RUNTIME_FLAVOR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "debug".to_owned())
}

fn release_channel() -> String {
    env::var("CODEX_ROUTER_RELEASE_CHANNEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "local".to_owned())
}

fn observability_marker() -> String {
    env::var(OBSERVABILITY_MARKER_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "none".to_owned())
}

fn sanitize_error(error: &str) -> String {
    Path::new(error)
        .file_name()
        .and_then(|value| value.to_str())
        .map_or_else(|| "redacted".to_owned(), ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn foreground_host_defaults_to_the_shared_loopback_collector() {
        assert_eq!(
            resolve_otlp_endpoint(None, TelemetryMode::ForegroundHost),
            Some("http://127.0.0.1:4318".to_owned())
        );
        assert_eq!(
            resolve_otlp_endpoint(None, TelemetryMode::EnvironmentOnly),
            None
        );
    }

    #[test]
    fn explicit_otlp_endpoint_overrides_the_foreground_host_default() {
        assert_eq!(
            resolve_otlp_endpoint(
                Some("http://127.0.0.1:14318/"),
                TelemetryMode::ForegroundHost
            ),
            Some("http://127.0.0.1:14318/".to_owned())
        );
        assert_eq!(
            foreground_host_otlp_endpoint(Some("http://127.0.0.1:14318/")),
            "http://127.0.0.1:14318/"
        );
    }

    #[test]
    fn signal_endpoints_append_only_their_own_otlp_path() {
        let endpoint = "http://127.0.0.1:4318";

        assert_eq!(trace_endpoint(endpoint), format!("{endpoint}/v1/traces"));
        assert_eq!(metric_endpoint(endpoint), format!("{endpoint}/v1/metrics"));
        assert_eq!(log_endpoint(endpoint), format!("{endpoint}/v1/logs"));
    }

    #[test]
    fn telemetry_shutdown_completion_is_shared_across_guard_and_handles() {
        let guard = TelemetryGuard {
            tracer_provider: None,
            meter_provider: None,
            logger_provider: None,
            completed: Arc::new(AtomicBool::new(false)),
        };
        let first_handle = guard.shutdown_handle();
        let second_handle = first_handle.clone();

        assert!(Arc::ptr_eq(&guard.completed, &first_handle.completed));
        assert!(Arc::ptr_eq(
            &first_handle.completed,
            &second_handle.completed
        ));

        first_handle.flush_and_shutdown();
        assert!(guard.completed.load(Ordering::Acquire));
        drop(guard);
        second_handle.flush_and_shutdown();
    }

    #[test]
    fn process_start_telemetry_does_not_emit_raw_otlp_endpoint() {
        let source = include_str!("telemetry.rs");
        let forbidden_raw_endpoint_field = ["otel.endpoint", " = ", "%endpoint"].concat();

        assert!(source.contains("otel.endpoint.configured = true"));
        assert!(!source.contains(&forbidden_raw_endpoint_field));
    }
}
