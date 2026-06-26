//! Runtime telemetry setup for the codex-router CLI.

use std::env;
use std::path::Path;
use std::process::Command;

use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::Protocol;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
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

#[derive(Debug)]
pub(crate) struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(tracer_provider) = self.tracer_provider.take() {
            let _ = tracer_provider.force_flush();
            let _ = tracer_provider.shutdown();
        }
        if let Some(meter_provider) = self.meter_provider.take() {
            let _ = meter_provider.force_flush();
            let _ = meter_provider.shutdown();
        }
    }
}

pub(crate) fn init_from_env() -> TelemetryGuard {
    let filter = env::var("RUST_LOG").unwrap_or_else(|_error| DEFAULT_LOG_FILTER.to_owned());
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .compact();
    let Some(endpoint) = otlp_endpoint() else {
        let _ = tracing_subscriber::registry()
            .with(EnvFilter::new(filter))
            .with(fmt_layer)
            .try_init();
        tracing::info!(
            service.name = SERVICE_NAME,
            service.version = env!("CARGO_PKG_VERSION"),
            "codex_router.process_start"
        );
        return TelemetryGuard {
            tracer_provider: None,
            meter_provider: None,
        };
    };

    let tracer_provider = match build_tracer_provider(&endpoint) {
        Ok(provider) => provider,
        Err(error) => {
            let _ = tracing_subscriber::registry()
                .with(EnvFilter::new(filter))
                .with(fmt_layer)
                .try_init();
            tracing::warn!(
                error.kind = "otel_init_failed",
                error = %sanitize_error(&error.to_string()),
                "codex_router.telemetry_init_failed"
            );
            return TelemetryGuard {
                tracer_provider: None,
                meter_provider: None,
            };
        }
    };
    let meter_provider = match build_meter_provider(&endpoint) {
        Ok(provider) => Some(provider),
        Err(error) => {
            tracing::warn!(
                error.kind = "otel_metrics_init_failed",
                error = %sanitize_error(&error.to_string()),
                "codex_router.telemetry_metrics_init_failed"
            );
            None
        }
    };
    if let Some(meter_provider) = meter_provider.clone() {
        global::set_meter_provider(meter_provider);
    }
    let tracer = tracer_provider.tracer(SERVICE_NAME);
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::new(filter))
        .with(fmt_layer)
        .with(otel_layer)
        .try_init();
    tracing::info!(
        service.name = SERVICE_NAME,
        service.version = env!("CARGO_PKG_VERSION"),
        otel.endpoint = %endpoint,
        "codex_router.process_start"
    );

    TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        meter_provider,
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

fn otlp_endpoint() -> Option<String> {
    let endpoint = env::var(OTLP_ENDPOINT_ENV).ok()?;
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        None
    } else {
        Some(endpoint.to_owned())
    }
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
    digest[..8]
        .iter()
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
