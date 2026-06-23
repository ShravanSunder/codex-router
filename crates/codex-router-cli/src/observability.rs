//! Debug OpenTelemetry producer wiring for local diagnostics.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use opentelemetry::KeyValue;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::Protocol;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const DEFAULT_OTEL_ENDPOINT: &str = "http://127.0.0.1:4318";
const DEFAULT_OTEL_HEALTH_URL: &str = "http://127.0.0.1:13133/";

/// Returns the shared local OTLP HTTP endpoint.
#[must_use]
pub fn default_otel_endpoint() -> String {
    DEFAULT_OTEL_ENDPOINT.to_owned()
}

/// Returns the shared local collector health URL.
#[must_use]
pub fn default_otel_health_url() -> String {
    DEFAULT_OTEL_HEALTH_URL.to_owned()
}

/// Debug OTEL initialization options.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugOtelConfig {
    /// Router root that owns debug marker/state files.
    pub router_root: PathBuf,
    /// Loopback OTLP HTTP base endpoint.
    pub endpoint: String,
    /// Loopback collector health URL.
    pub health_url: String,
    /// Redacted audit JSONL path used by the same debug run.
    pub audit_file: Option<PathBuf>,
}

/// Guard that flushes OTEL logs at process shutdown.
#[derive(Debug)]
pub struct DebugOtelGuard {
    logger_provider: SdkLoggerProvider,
}

impl Drop for DebugOtelGuard {
    fn drop(&mut self) {
        let _result = self.logger_provider.shutdown();
    }
}

/// Initializes strict debug OTEL for the local managed Victoria stack.
pub fn init_debug_otel(config: DebugOtelConfig) -> Result<DebugOtelGuard, ObservabilityError> {
    validate_loopback_url("--otel-endpoint", &config.endpoint)?;
    validate_loopback_url("--otel-health-url", &config.health_url)?;
    check_collector_health(&config.health_url)?;

    let marker = format!("codex-router-{}", current_unix_seconds());
    let query_start = current_unix_seconds().to_string();
    let state_file = write_state_file(&config.router_root, &marker, &query_start)?;
    let resource = build_resource(&config.router_root);
    let log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_endpoint(signal_endpoint(&config.endpoint, "v1/logs"))
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(Duration::from_secs(2))
        .build()
        .map_err(|source| ObservabilityError::ExporterBuild {
            message: source.to_string(),
        })?;
    let logger_provider = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(log_exporter)
        .build();
    let filter = EnvFilter::new(
        "codex_router=debug,codex_router_core=debug,codex_router_proxy=debug,codex_router_cli=debug,opentelemetry=off,reqwest=off,hyper=off,h2=off",
    );
    let otel_layer = OpenTelemetryTracingBridge::new(&logger_provider).with_filter(filter);
    tracing_subscriber::registry()
        .with(otel_layer)
        .try_init()
        .map_err(|source| ObservabilityError::SubscriberInit {
            message: source.to_string(),
        })?;

    tracing::info!(
        target: "codex_router.observability",
        marker = marker,
        query_start = query_start,
        state_file_hash = hash_path(&state_file),
        audit_file_hash = config.audit_file.as_deref().map(hash_path).unwrap_or_default(),
        "codex_router_debug_otel_started"
    );

    Ok(DebugOtelGuard { logger_provider })
}

fn build_resource(router_root: &Path) -> Resource {
    Resource::builder_empty()
        .with_service_name("codex-router")
        .with_attributes([
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new("dev.repo.hash", repo_hash()),
            KeyValue::new("dev.worktree.hash", hash_path(router_root)),
            KeyValue::new("dev.branch.name", current_branch_name()),
            KeyValue::new("dev.runtime.flavor", "debug"),
            KeyValue::new("dev.release.channel", "local"),
        ])
        .build()
}

fn check_collector_health(health_url: &str) -> Result<(), ObservabilityError> {
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(ObservabilityError::HealthClient)?
        .get(health_url)
        .send()
        .map_err(ObservabilityError::HealthRequest)?;
    if response.status().is_success() {
        return Ok(());
    }

    Err(ObservabilityError::CollectorUnhealthy {
        status: response.status().as_u16(),
    })
}

fn validate_loopback_url(option: &'static str, value: &str) -> Result<(), ObservabilityError> {
    let allowed = value.starts_with("http://127.0.0.1:")
        || value.starts_with("http://localhost:")
        || value.starts_with("http://[::1]:");
    if allowed {
        return Ok(());
    }

    Err(ObservabilityError::NonLoopbackEndpoint {
        option,
        value: value.to_owned(),
    })
}

fn signal_endpoint(endpoint: &str, path: &str) -> String {
    format!("{}/{}", endpoint.trim_end_matches('/'), path)
}

fn write_state_file(
    router_root: &Path,
    marker: &str,
    query_start: &str,
) -> Result<PathBuf, ObservabilityError> {
    let debug_dir = router_root.join("debug");
    fs::create_dir_all(&debug_dir).map_err(|source| ObservabilityError::StateFile {
        path: debug_dir.clone(),
        source,
    })?;
    set_private_dir_permissions(&debug_dir)?;
    let state_file = debug_dir.join("otel-state.env");
    let mut file =
        fs::File::create(&state_file).map_err(|source| ObservabilityError::StateFile {
            path: state_file.clone(),
            source,
        })?;
    writeln!(file, "CODEX_ROUTER_OBSERVABILITY_MARKER={marker}").map_err(|source| {
        ObservabilityError::StateFile {
            path: state_file.clone(),
            source,
        }
    })?;
    writeln!(file, "CODEX_ROUTER_OBSERVABILITY_QUERY_START={query_start}").map_err(|source| {
        ObservabilityError::StateFile {
            path: state_file.clone(),
            source,
        }
    })?;
    set_private_file_permissions(&state_file)?;

    Ok(state_file)
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<(), ObservabilityError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        ObservabilityError::StateFile {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<(), ObservabilityError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), ObservabilityError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        ObservabilityError::StateFile {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), ObservabilityError> {
    Ok(())
}

fn repo_hash() -> String {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    hash_path(&repo_root)
}

fn hash_path(path: &Path) -> String {
    hash_text(&path.to_string_lossy())
}

fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn current_branch_name() -> String {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        _other => "unknown".to_owned(),
    }
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Debug observability setup error.
#[derive(Debug, Error)]
pub enum ObservabilityError {
    /// Explicit debug OTEL only permits loopback endpoints.
    #[error("{option} must be a loopback HTTP URL, got {value}")]
    NonLoopbackEndpoint {
        /// Option name.
        option: &'static str,
        /// Rejected value.
        value: String,
    },
    /// Collector returned an unhealthy status.
    #[error(
        "OTEL collector is not healthy: HTTP {status}; run ~/dev/ai-tools/observability/observability-stack up"
    )]
    CollectorUnhealthy {
        /// HTTP status.
        status: u16,
    },
    /// Collector health request failed.
    #[error(
        "failed checking OTEL collector health; run ~/dev/ai-tools/observability/observability-stack up"
    )]
    HealthRequest(#[source] reqwest::Error),
    /// Collector health client could not be built.
    #[error("failed building OTEL collector health client")]
    HealthClient(#[source] reqwest::Error),
    /// State file could not be written.
    #[error("failed writing debug OTEL state file at {}: {source}", path.display())]
    StateFile {
        /// Path that failed.
        path: PathBuf,
        /// Source error.
        source: std::io::Error,
    },
    /// OTLP exporter could not be built.
    #[error("failed building OTLP log exporter: {message}")]
    ExporterBuild {
        /// Redacted message.
        message: String,
    },
    /// Tracing subscriber could not be installed.
    #[error("failed initializing tracing subscriber: {message}")]
    SubscriberInit {
        /// Redacted message.
        message: String,
    },
}
