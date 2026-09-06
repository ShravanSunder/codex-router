//! WebSocket registry report serialization and file validation.
use crate::CliError;
use codex_router_proxy::server::LoopbackRouterRuntime;
use std::fs;
use std::path::PathBuf;

pub(super) fn write_websocket_registry_report_file(
    report_file: &PathBuf,
    handled_connections: usize,
    runtime: &LoopbackRouterRuntime,
) -> Result<(), CliError> {
    if let Some(parent) = report_file.parent() {
        fs::create_dir_all(parent).map_err(|source| CliError::WebSocketRegistryReportWrite {
            path: parent.display().to_string(),
            source,
        })?;
    }
    let snapshot = runtime.websocket_registry_snapshot();
    let report = websocket_registry_report_value(handled_connections, &snapshot);
    let rendered =
        serde_json::to_vec_pretty(&report).map_err(CliError::WebSocketRegistryReportRender)?;
    fs::write(report_file, rendered).map_err(|source| CliError::WebSocketRegistryReportWrite {
        path: report_file.display().to_string(),
        source,
    })
}

pub(super) fn websocket_registry_report_value(
    handled_connections: usize,
    snapshot: &codex_router_proxy::websocket::WebSocketRegistrySnapshot,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 2,
        "handled_connections": handled_connections,
        "websocket_registry": {
            "active_sessions": snapshot.active_sessions,
            "high_water_sessions": snapshot.high_water_sessions,
            "registered_sessions": snapshot.registered_sessions,
            "closed_sessions": snapshot.closed_sessions,
            "completed_response_sessions": snapshot.completed_response_sessions,
            "forwarded_upstream_messages": snapshot.forwarded_upstream_messages,
            "registered_session_id_count": snapshot.registered_session_ids.len(),
            "completed_session_id_count": snapshot.completed_session_ids.len(),
            "closed_session_id_count": snapshot.closed_session_ids.len(),
            "session_peer_addr_count": snapshot.session_peer_addrs.len(),
            "session_peer_join_observable": !snapshot.session_peer_addrs.is_empty(),
            "completed_session_forwarded_upstream_message_counts": snapshot.completed_session_forwarded_upstream_message_counts,
            "final_session_forwarded_upstream_message_counts": snapshot.final_session_forwarded_upstream_message_counts,
            "quota_reconnect_signal_count": snapshot.quota_reconnect_signal_count,
            "quota_reconnect_signal_unix_ms": snapshot.quota_reconnect_signal_unix_ms,
        },
    })
}

pub(super) fn validate_websocket_registry_report_file(
    report_file: &PathBuf,
) -> Result<(), CliError> {
    if let Some(parent) = report_file.parent() {
        fs::create_dir_all(parent).map_err(|source| CliError::WebSocketRegistryReportWrite {
            path: parent.display().to_string(),
            source,
        })?;
    }
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(report_file)
        .and_then(|file| file.sync_all())
        .map_err(|source| CliError::WebSocketRegistryReportWrite {
            path: report_file.display().to_string(),
            source,
        })
}
