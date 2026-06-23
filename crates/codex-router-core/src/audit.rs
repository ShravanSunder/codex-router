//! Redacted audit event schema.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

use serde::Serialize;
use thiserror::Error;

use crate::ids::RequestId;
use crate::redaction::SecretString;

/// Supported route families for audit events.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    /// `/v1/responses`.
    Responses,
    /// `/v1/models`.
    Models,
    /// `/v1/responses/compact`.
    Compact,
    /// `/v1/memories/trace_summarize`.
    MemoryTrace,
    /// Responses WebSocket.
    ResponsesWebSocket,
}

/// Local decision outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    /// Request was rejected locally.
    Rejected,
    /// Request was allowed to continue.
    Allowed,
}

/// Transport family for audit events.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    /// HTTP or HTTP/SSE request.
    Http,
    /// WebSocket connection.
    WebSocket,
}

/// Local auth outcome without token material.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAuthAuditResult {
    /// Local token was valid.
    Valid,
    /// Local token was missing.
    Missing,
    /// Local token was empty.
    Empty,
    /// Local token was from an old generation.
    Old,
    /// Local token was wrong.
    Wrong,
}

/// Whether the upstream response had been committed to the local client.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseCommitState {
    /// No upstream/local response had been committed.
    NotCommitted,
    /// Response was committed.
    Committed,
}

/// Audit event payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuditEvent {
    event_type: &'static str,
    request_id: RequestId,
    route_kind: RouteKind,
    transport_kind: TransportKind,
    local_auth_result: LocalAuthAuditResult,
    outcome: AuditOutcome,
    decision_reason: &'static str,
    response_commit_state: ResponseCommitState,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_class: Option<&'static str>,
}

impl AuditEvent {
    /// Creates a local-auth rejection event.
    #[must_use]
    pub fn local_auth_rejected(
        request_id: RequestId,
        route_kind: RouteKind,
        outcome: AuditOutcome,
        reason: SecretString,
    ) -> Self {
        let _redacted_reason = reason;
        Self {
            event_type: "proxy_decision",
            request_id,
            route_kind,
            transport_kind: TransportKind::Http,
            local_auth_result: LocalAuthAuditResult::Missing,
            outcome,
            decision_reason: "local_auth_rejected",
            response_commit_state: ResponseCommitState::NotCommitted,
            account_hash: None,
            error_class: Some("local_auth"),
        }
    }

    /// Creates a proxy decision event from allowlisted fields.
    #[must_use]
    pub fn proxy_decision(fields: AuditEventFields) -> Self {
        Self {
            event_type: "proxy_decision",
            request_id: fields.request_id,
            route_kind: fields.route_kind,
            transport_kind: fields.transport_kind,
            local_auth_result: fields.local_auth_result,
            outcome: fields.outcome,
            decision_reason: fields.decision_reason,
            response_commit_state: fields.response_commit_state,
            account_hash: fields.account_hash,
            error_class: fields.error_class,
        }
    }

    /// Emits the safe audit fields through `tracing` when a subscriber is installed.
    pub fn emit_tracing_event(&self) {
        tracing::info!(
            target: "codex_router.audit",
            event_type = self.event_type,
            request_id = self.request_id.as_str(),
            route_kind = ?self.route_kind,
            transport_kind = ?self.transport_kind,
            local_auth_result = ?self.local_auth_result,
            outcome = ?self.outcome,
            decision_reason = self.decision_reason,
            response_commit_state = ?self.response_commit_state,
            account_hash = self.account_hash.as_deref().unwrap_or(""),
            error_class = self.error_class.unwrap_or(""),
            "codex_router_proxy_decision"
        );
    }
}

/// Construction fields for an audit event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditEventFields {
    /// Request id.
    pub request_id: RequestId,
    /// Route kind.
    pub route_kind: RouteKind,
    /// Transport kind.
    pub transport_kind: TransportKind,
    /// Local auth result.
    pub local_auth_result: LocalAuthAuditResult,
    /// Outcome.
    pub outcome: AuditOutcome,
    /// Safe static decision reason.
    pub decision_reason: &'static str,
    /// Response commit state.
    pub response_commit_state: ResponseCommitState,
    /// Redacted account hash.
    pub account_hash: Option<String>,
    /// Safe static error class.
    pub error_class: Option<&'static str>,
}

/// File-backed audit sink.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFileSink {
    path: PathBuf,
}

impl AuditFileSink {
    /// Creates a file-backed audit sink.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns audit file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one JSONL audit event.
    pub fn append(&self, event: &AuditEvent) -> Result<(), AuditSinkError> {
        event.emit_tracing_event();
        if let Some(parent) = self.path.parent() {
            create_private_dir(parent)?;
        }
        let mut file = open_private_append_file(&self.path)?;
        let line = serde_json::to_string(event).map_err(AuditSinkError::Serialize)?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .map_err(|source| AuditSinkError::Filesystem {
                path: self.path.clone(),
                source,
            })?;
        sync_file(&file, &self.path)
    }
}

/// Audit sink error.
#[derive(Debug, Error)]
pub enum AuditSinkError {
    /// Event could not serialize.
    #[error("failed serializing audit event: {0}")]
    Serialize(serde_json::Error),
    /// Filesystem operation failed.
    #[error("audit filesystem error at {}: {source}", path.display())]
    Filesystem {
        /// Path that failed.
        path: PathBuf,
        /// Source error.
        source: std::io::Error,
    },
}

fn create_private_dir(path: &Path) -> Result<(), AuditSinkError> {
    fs::create_dir_all(path).map_err(|source| AuditSinkError::Filesystem {
        path: path.to_path_buf(),
        source,
    })?;
    set_private_dir_permissions(path)
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<(), AuditSinkError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        AuditSinkError::Filesystem {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<(), AuditSinkError> {
    Ok(())
}

#[cfg(unix)]
fn open_private_append_file(path: &Path) -> Result<fs::File, AuditSinkError> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| AuditSinkError::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn open_private_append_file(path: &Path) -> Result<fs::File, AuditSinkError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| AuditSinkError::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

fn sync_file(file: &fs::File, path: &Path) -> Result<(), AuditSinkError> {
    file.sync_all()
        .map_err(|source| AuditSinkError::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}
